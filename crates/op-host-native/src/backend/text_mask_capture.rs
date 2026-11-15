//! One lexical prepared text mask per render thread. Record the real glyph/
//! decoration draws with their original device CTM and replay at identity.
use super::NativeFrameBackend;
use op_editor_ui::{
    Color, Point2D, Rect, TextDecorationRequest, TextLayout, TextMaskAction as A,
    TextMaskResult as R,
};
use skia_safe::{Canvas, IRect, Matrix, Picture, PictureRecorder};
use std::{cell::RefCell, rc::Rc};
const LIMIT: usize = 16 * 1024 * 1024;
#[derive(Default)]
struct Observation {
    recorder: usize,
    bytes: usize,
    unproven: bool,
    exceeded: bool,
}
struct Recording {
    owner: usize,
    matrix: Matrix,
    clip: IRect,
    generation: u64,
    recorder: PictureRecorder,
    note: Rc<RefCell<Observation>>,
}
struct Prepared {
    owner: usize,
    matrix: Matrix,
    clip: IRect,
    generation: u64,
    picture: Picture,
}
enum Session {
    Recording(Recording),
    Prepared(Prepared),
    Discarded(usize),
}
thread_local! {
    static CURRENT:RefCell<Option<Session>>=const{RefCell::new(None)};
    static NOTE:RefCell<Option<Rc<RefCell<Observation>>>>=const{RefCell::new(None)};
}
fn identity(canvas: &Canvas) -> usize {
    canvas as *const Canvas as usize
}
fn same(a: &Matrix, b: &Matrix) -> bool {
    let (mut x, mut y) = ([0.0; 9], [0.0; 9]);
    a.get_9(&mut x);
    b.get_9(&mut y);
    x.map(f32::to_bits) == y.map(f32::to_bits)
}
impl Session {
    fn owner(&self) -> usize {
        match self {
            Self::Recording(r) => r.owner,
            Self::Prepared(p) => p.owner,
            Self::Discarded(id) => *id,
        }
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        NOTE.with(|slot| {
            let ours = slot
                .borrow()
                .as_ref()
                .is_some_and(|n| Rc::ptr_eq(n, &self.note));
            if ours {
                slot.borrow_mut().take();
            }
        });
    }
}
fn note(canvas: &Canvas, action: impl FnOnce(&mut Observation)) {
    NOTE.with(|slot| {
        if let Some(note) = slot.borrow().as_ref() {
            let mut note = note.borrow_mut();
            if note.recorder == identity(canvas) {
                action(&mut note);
            }
        }
    });
}
fn charge(note: &mut Observation, bytes: usize) {
    note.bytes = note.bytes.saturating_add(bytes);
    note.exceeded |= note.bytes > LIMIT;
}
pub(super) fn unproven(canvas: &Canvas) {
    note(canvas, |n| n.unproven = true);
}
pub(super) fn skip(canvas: &Canvas) -> bool {
    let mut skip = false;
    note(canvas, |n| skip = n.exceeded);
    skip
}
/// Conservatively charge the immutable A16 coverage image referenced by the
/// recorded clip shader, including metadata/sharing slack. No payload copy.
pub(super) fn coverage(canvas: &Canvas, bounds: skia_safe::Rect) -> bool {
    let mut keep = true;
    note(canvas, |n| {
        let (device, _) = canvas.local_to_device_as_3x3().map_rect(bounds);
        let bytes = (device.width().ceil().max(0.0) as usize)
            .saturating_mul(device.height().ceil().max(0.0) as usize)
            .saturating_mul(4)
            .saturating_add(1024);
        charge(n, bytes);
        keep = !n.exceeded;
    });
    keep
}
fn take(owner: usize) -> Option<Session> {
    CURRENT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|s| s.owner() == owner) {
            slot.take()
        } else {
            None
        }
    })
}
fn store(s: Session) {
    CURRENT.with(|slot| *slot.borrow_mut() = Some(s));
}
fn with_canvas<T>(
    original: &Canvas,
    suppressed: T,
    bytes: usize,
    action: impl FnOnce(&Canvas) -> T,
) -> T {
    let Some(session) = take(identity(original)) else {
        return action(original);
    };
    match session {
        Session::Recording(mut r) => {
            charge(&mut r.note.borrow_mut(), bytes);
            if r.note.borrow().exceeded {
                store(Session::Discarded(r.owner));
                return suppressed;
            }
            let result = action(r.recorder.recording_canvas().expect("active mask recorder"));
            if r.note.borrow().exceeded {
                store(Session::Discarded(r.owner));
            } else {
                store(Session::Recording(r));
            }
            result
        }
        Session::Discarded(owner) => {
            store(Session::Discarded(owner));
            suppressed
        }
        session => {
            store(session);
            action(original)
        }
    }
}
pub(super) fn draw_text(frame: &mut NativeFrameBackend<'_>, layout: &TextLayout, origin: Point2D) {
    let bytes = layout.runs().iter().fold(1024usize, |sum, r| {
        sum.saturating_add(r.content.len().saturating_mul(512))
    });
    with_canvas(frame.canvas, (), bytes, |canvas| {
        frame.inner.draw_text(canvas, layout, origin)
    });
}
pub(super) fn decoration(
    frame: &mut NativeFrameBackend<'_>,
    request: &TextDecorationRequest<'_>,
) -> bool {
    with_canvas(frame.canvas, true, 1024, |canvas| {
        frame.inner.draw_text_decoration(canvas, request)
    })
}
pub(super) fn stroke_line(
    frame: &mut NativeFrameBackend<'_>,
    from: Point2D,
    to: Point2D,
    color: Color,
    width: f32,
) {
    with_canvas(frame.canvas, (), 1024, |canvas| {
        unproven(canvas);
        frame.inner.stroke_line(canvas, from, to, color, width);
    });
}
pub(super) fn command(canvas: &Canvas, action: A) -> R {
    match action {
        A::Begin => begin(canvas),
        A::Finish => finish(canvas),
        A::Replay => replay(canvas),
        A::Discard => {
            drop(take(identity(canvas)));
            R::Unavailable
        }
    }
}
fn begin(canvas: &Canvas) -> R {
    if CURRENT.with(|s| s.borrow().is_some()) || NOTE.with(|s| s.borrow().is_some()) {
        return R::Unavailable;
    }
    // Perspective keeps the established128 path. Device raster-mask support
    // is observed below rather than inferred from font size/line boxes.
    let matrix = canvas.local_to_device_as_3x3();
    if !matrix.is_finite() || matrix.has_perspective() || matrix.invert().is_none() {
        return R::Unavailable;
    }
    let Some(clip) = canvas.device_clip_bounds() else {
        return R::Unavailable;
    };
    let mut recorder = PictureRecorder::new();
    let recording = recorder.begin_recording(skia_safe::Rect::from_irect(clip), true);
    recording.concat(&matrix);
    if !same(&matrix, &recording.local_to_device_as_3x3())
        || recording.device_clip_bounds() != Some(clip)
    {
        return R::Unavailable;
    }
    let note = Rc::new(RefCell::new(Observation {
        recorder: identity(recording),
        ..Default::default()
    }));
    NOTE.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&note)));
    store(Session::Recording(Recording {
        owner: identity(canvas),
        matrix,
        clip,
        generation: jian_skia::font_generation(),
        recorder,
        note,
    }));
    R::Recording
}
fn finish(canvas: &Canvas) -> R {
    let Some(Session::Recording(mut r)) = take(identity(canvas)) else {
        return R::Unavailable;
    };
    let Some(picture) = r.recorder.finish_recording_as_picture(None) else {
        return R::Unavailable;
    };
    let note = r.note.borrow();
    if note.exceeded
        || note.bytes.saturating_add(picture.approximate_bytes_used()) > LIMIT
        || r.generation != jian_skia::font_generation()
    {
        return R::Unavailable;
    }
    let bounds = if note.unproven {
        None
    } else {
        local_support(&r.matrix, r.clip, picture.cull_rect())
    };
    store(Session::Prepared(Prepared {
        owner: r.owner,
        matrix: r.matrix.clone(),
        clip: r.clip,
        generation: r.generation,
        picture,
    }));
    R::Ready(bounds)
}
fn local_support(matrix: &Matrix, clip: IRect, mut bounds: skia_safe::Rect) -> Option<Rect> {
    if bounds.is_empty() {
        return Some(Rect::ZERO);
    }
    if !bounds.is_finite() {
        return None;
    }
    // Raster support occupies complete device pixel cells. Round the actual
    // recorded draw support out before inverse mapping, including AA coverage.
    bounds = skia_safe::Rect::from_irect(bounds.round_out());
    if !bounds.intersect(skia_safe::Rect::from_irect(clip)) {
        return Some(Rect::ZERO);
    }
    let device = bounds;
    let inverse = matrix.invert()?;
    let (bounds, _) = inverse.map_rect(device);
    if !bounds.is_finite() {
        return None;
    }
    let (l, t, r, b) = (
        bounds.left.next_down(),
        bounds.top.next_down(),
        bounds.right.next_up(),
        bounds.bottom.next_up(),
    );
    let rect = Rect::xywh(l, t, (r - l).next_up(), (b - t).next_up());
    if ![
        rect.origin.x,
        rect.origin.y,
        rect.size.x,
        rect.size.y,
        rect.origin.x + rect.size.x,
        rect.origin.y + rect.size.y,
    ]
    .into_iter()
    .all(f32::is_finite)
    {
        return None;
    }
    // Use the same Skia map as saveLayer; ill-conditioned inverse rounding
    // must never make the allocation hint smaller than the observed pixels.
    let (mapped, _) = matrix.map_rect(skia_safe::Rect::from_xywh(
        rect.origin.x,
        rect.origin.y,
        rect.size.x,
        rect.size.y,
    ));
    (mapped.left <= device.left
        && mapped.top <= device.top
        && mapped.right >= device.right
        && mapped.bottom >= device.bottom)
        .then_some(rect)
}
fn replay(canvas: &Canvas) -> R {
    let Some(Session::Prepared(p)) = take(identity(canvas)) else {
        return R::Unavailable;
    };
    // Refusal is checked BEFORE any draw. The caller can discard its empty
    // DstIn layer and repaint the full128 path without exposing partial ink.
    let clip = canvas.device_clip_bounds();
    let expanded = clip.is_some_and(|c| {
        c.left < p.clip.left
            || c.top < p.clip.top
            || c.right > p.clip.right
            || c.bottom > p.clip.bottom
    });
    if p.generation != jian_skia::font_generation()
        || expanded
        || !same(&p.matrix, &canvas.local_to_device_as_3x3())
    {
        return R::Unavailable;
    }
    let saved = canvas.save();
    canvas.reset_matrix();
    canvas.draw_picture(&p.picture, None, None);
    canvas.restore_to_count(saved);
    R::Replayed
}
