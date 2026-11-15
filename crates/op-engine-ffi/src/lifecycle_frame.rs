//! GPU presentation and bounded single-surface CPU readback for the C ABI.
#[cfg(feature = "editor")]
use super::earliest_wake;
use super::{Session, SurfaceSlot};
use crate::{
    error::{FfiError, FfiResult},
    render::paint_frame,
    OpStatus,
};

pub(super) struct CpuFrame {
    width: i32,
    height: i32,
    surface: jian_skia::SkiaSurface,
}

impl Session {
    /// Pump and present one GPU frame. `Ok(())` means painted (or the
    /// drawable was unavailable and a redraw is queued).
    pub(crate) fn frame_gpu(&mut self, now_ms: u64) -> FfiResult<()> {
        // A platform surface callback can already be queued when the shell
        // synchronously suspends. Reject it before advancing any editor
        // runtime work: suspended generation is owned exclusively by the
        // render-free `op_background_tick` path.
        if self.suspended {
            return Err(FfiError::new(OpStatus::Suspended, "engine is suspended"));
        }
        self.advance_global_clock(now_ms);
        self.pump_editor_preview();
        #[cfg(feature = "editor")]
        crate::editor_template::drain_pending_scene_template(self)?;
        #[cfg(feature = "editor")]
        let auth_wake = crate::editor_auth::pump(self);
        #[cfg(feature = "editor")]
        let model_discovery_wake = self.pump_editor_model_discovery(now_ms);
        #[cfg(feature = "editor")]
        let chat_wake = self.pump_editor_chat(now_ms);
        #[cfg(feature = "editor")]
        let codegen_wake = self.pump_editor_codegen(now_ms);
        #[cfg(feature = "editor")]
        let image_search_wake = self.pump_editor_image_search(now_ms);
        #[cfg(feature = "editor")]
        let collab_wake = self.pump_editor_collab();
        let slot = std::mem::replace(&mut self.surface, SurfaceSlot::None);
        let (restored, outcome) = match slot {
            SurfaceSlot::None => (
                None,
                Err(FfiError::new(
                    OpStatus::NotReady,
                    "no GPU surface attached (call op_attach_surface)",
                )),
            ),
            #[cfg(all(feature = "metal", any(target_os = "macos", target_os = "ios")))]
            SurfaceSlot::Metal(mut metal) => {
                let result = metal.draw_frame(|skia| paint_frame(self, skia));
                (
                    Some(SurfaceSlot::Metal(metal)),
                    result.map_err(|e| FfiError::new(OpStatus::GpuError, e)),
                )
            }
            #[cfg(all(feature = "gl", any(target_os = "android", target_env = "ohos")))]
            SurfaceSlot::Egl(mut egl) => {
                let result = egl.draw_frame(|skia| paint_frame(self, skia));
                if let Err(e) = &result {
                    eprintln!("op-engine-ffi: EGL draw_frame error: {e}");
                }
                (
                    Some(SurfaceSlot::Egl(egl)),
                    result.map_err(|e| FfiError::new(OpStatus::GpuError, e.to_string())),
                )
            }
        };
        if let Some(surface) = restored {
            self.surface = surface;
        }
        match outcome {
            Ok(true) => {
                crate::media::drain_remote_image_requests(self);
                #[cfg(feature = "editor")]
                if let Some(deadline) = earliest_wake(
                    earliest_wake(
                        earliest_wake(auth_wake, model_discovery_wake),
                        earliest_wake(chat_wake, codegen_wake),
                    ),
                    earliest_wake(
                        earliest_wake(collab_wake, image_search_wake),
                        self.editor
                            .as_ref()
                            .and_then(|host| host.next_animation_deadline_ms()),
                    ),
                ) {
                    self.request_wake(Some(deadline));
                }
                #[cfg(not(feature = "editor"))]
                self.schedule_caret_blink();
                Ok(())
            }
            // Drawable unavailable (Metal `nextDrawable` nil / EGL
            // BadAccess): leave the dirty bit set and ask the shell to
            // retry promptly.
            Ok(false) => {
                self.request_redraw();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Pump and copy one CPU frame into caller-owned RGBA8888 storage.
    ///
    /// # Safety
    ///
    /// `buffer` must cover `buffer_len` writable bytes and
    /// `stride >= physical_width * 4`.
    pub(crate) fn frame_cpu(
        &mut self,
        now_ms: u64,
        buffer: *mut u8,
        buffer_len: usize,
        stride: usize,
    ) -> FfiResult<()> {
        self.advance_global_clock(now_ms);
        self.pump_editor_preview();
        #[cfg(feature = "editor")]
        crate::editor_template::drain_pending_scene_template(self)?;
        #[cfg(feature = "editor")]
        let auth_wake = crate::editor_auth::pump(self);
        #[cfg(feature = "editor")]
        let model_discovery_wake = self.pump_editor_model_discovery(now_ms);
        #[cfg(feature = "editor")]
        let chat_wake = self.pump_editor_chat(now_ms);
        #[cfg(feature = "editor")]
        let codegen_wake = self.pump_editor_codegen(now_ms);
        #[cfg(feature = "editor")]
        let image_search_wake = self.pump_editor_image_search(now_ms);
        #[cfg(feature = "editor")]
        let collab_wake = self.pump_editor_collab();
        let width = ((self.logical.0 * self.dpr).round() as i32).max(1);
        let height = ((self.logical.1 * self.dpr).round() as i32).max(1);
        let row_bytes = width as usize * 4;
        if stride < row_bytes {
            return Err(FfiError::invalid(
                "frame stride is smaller than the physical row",
            ));
        }
        let required = (height as usize)
            .checked_mul(stride)
            .filter(|required| *required <= isize::MAX as usize)
            .ok_or_else(|| {
                FfiError::invalid("frame stride and height exceed addressable storage")
            })?;
        if buffer.is_null() {
            return Err(FfiError::invalid("frame buffer pointer is null"));
        }
        if buffer_len < required {
            return Err(FfiError::invalid(format!(
                "frame buffer covers {buffer_len} bytes but {required} are required"
            )));
        }
        // Retain one raster for this session and exact display size. A warm
        // CPU frame allocates neither another surface nor a full pixel Vec.
        let mut frame = self
            .cpu_frame
            .take()
            .filter(|frame| frame.width == width && frame.height == height)
            .unwrap_or_else(|| CpuFrame {
                width,
                height,
                surface: jian_skia::SkiaSurface::new_raster(width, height),
            });
        paint_frame(self, &mut frame.surface);
        crate::media::drain_remote_image_requests(self);
        // SAFETY: the ABI caller provides writable storage, checked above for
        // null, stride, complete row coverage and Rust slice addressability.
        // The renderer owns its pixels separately from this caller buffer.
        let copied = unsafe {
            let destination = std::slice::from_raw_parts_mut(buffer, required);
            frame.surface.read_rgba8_strided(destination, stride)
        };
        self.cpu_frame = Some(frame);
        if !copied {
            return Err(FfiError::new(
                OpStatus::GpuError,
                "CPU frame readback failed",
            ));
        }
        #[cfg(feature = "editor")]
        {
            if let Some(deadline) = earliest_wake(
                earliest_wake(
                    earliest_wake(auth_wake, model_discovery_wake),
                    earliest_wake(chat_wake, codegen_wake),
                ),
                earliest_wake(
                    earliest_wake(collab_wake, image_search_wake),
                    self.editor
                        .as_ref()
                        .and_then(|host| host.next_animation_deadline_ms()),
                ),
            ) {
                self.request_wake(Some(deadline));
            }
        }
        #[cfg(not(feature = "editor"))]
        self.schedule_caret_blink();
        Ok(())
    }
}
