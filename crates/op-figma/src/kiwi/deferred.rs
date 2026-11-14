//! Validated component caches backed by immutable owned wire ranges.
//! Full decoding stays eager. Import reads hydrate once; unique mutation moves
//! ownership and shared mutation clones only the decoded value.
use super::{ByteBuffer, DecodeScope, Decoder, FigKey, FigValue, KiwiError, ResolvedType, Schema};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

struct Apply {
    function: Arc<dyn Fn(&mut FigValue) + Send + Sync>,
    extra: usize,
}
impl Apply {
    fn bytes(&self) -> usize {
        16 + std::mem::size_of_val(&*self.function) + self.extra
    }
}

pub(crate) struct Shared {
    schema: Schema,
    field_types: Arc<Vec<Vec<Option<ResolvedType>>>>,
    field_by_value: Arc<Vec<HashMap<u32, usize>>>,
    object_keys: Arc<Vec<Vec<FigKey>>>,
    compact_guids: Arc<Vec<Option<bool>>>,
    compact_affines: Arc<Vec<Option<[u8; 6]>>>,
    compact_floats: Arc<Vec<Option<(super::compact_float::FloatKind, [u8; 4])>>>,
}
#[derive(Clone, Copy)]
struct Spec {
    resolved: ResolvedType,
    array: bool,
    depth: u32,
    scope: DecodeScope,
    trailing: usize,
}
enum Pending {
    Wire {
        shared: Arc<Shared>,
        bytes: Arc<[u8]>,
        spec: Spec,
        apply: Vec<Apply>,
    },
    Mapped {
        source: Arc<Deferred>,
        apply: Vec<Apply>,
    },
}
impl Pending {
    fn decode(self) -> FigValue {
        let (mut value, apply) = match self {
            Self::Wire {
                shared,
                bytes,
                spec,
                apply,
            } => {
                let mut decoder = Decoder {
                    schema: &shared.schema,
                    bb: ByteBuffer::with_tail(&bytes, spec.trailing),
                    field_types: shared.field_types.clone(),
                    field_by_value: shared.field_by_value.clone(),
                    object_keys: shared.object_keys.clone(),
                    compact_guids: shared.compact_guids.clone(),
                    compact_affines: shared.compact_affines.clone(),
                    compact_floats: shared.compact_floats.clone(),
                    lazy_context: None,
                };
                let value = decoder
                    .decode_field(spec.resolved, spec.array, spec.depth, spec.scope)
                    .expect("immutable range was validated before deferral");
                assert_eq!(decoder.bb.remaining(), 0, "validated field range");
                (value, apply)
            }
            Self::Mapped { source, apply } => (source.get().clone(), apply),
        };
        for transform in apply {
            (transform.function)(&mut value);
        }
        value
    }
    fn apply(&mut self, transform: Apply) {
        match self {
            Self::Wire { apply, .. } | Self::Mapped { apply, .. } => apply.push(transform),
        }
    }
}
pub struct Deferred {
    pending: Mutex<Option<Pending>>,
    value: OnceLock<FigValue>,
}
impl std::fmt::Debug for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deferred")
            .field("ready", &self.value.get().is_some())
            .finish()
    }
}
impl Deferred {
    fn new(pending: Pending) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(Some(pending)),
            value: OnceLock::new(),
        })
    }
    pub fn get(&self) -> &FigValue {
        self.value.get_or_init(|| {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take()
                .expect("pending validated range")
                .decode()
        })
    }
    pub(crate) fn into_value(self) -> FigValue {
        if let Some(value) = self.value.into_inner() {
            value
        } else {
            self.pending
                .into_inner()
                .unwrap_or_else(|p| p.into_inner())
                .expect("pending validated range")
                .decode()
        }
    }
    pub(crate) fn ready(&self) -> Option<&FigValue> {
        self.value.get()
    }
    pub(crate) fn map(
        value: &mut Arc<Self>,
        extra: usize,
        transform: impl Fn(&mut FigValue) + Send + Sync + 'static,
    ) {
        let transform = Apply {
            function: Arc::new(transform),
            extra,
        };
        if let Some(unique) = Arc::get_mut(value) {
            if let Some(ready) = unique.value.get_mut() {
                (transform.function)(ready)
            } else {
                unique
                    .pending
                    .get_mut()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_mut()
                    .unwrap()
                    .apply(transform)
            }
        } else {
            *value = Self::new(Pending::Mapped {
                source: value.clone(),
                apply: vec![transform],
            });
        }
    }
    pub(crate) fn retained_bound(&self, owned: &impl Fn(&FigValue) -> usize) -> usize {
        let own = std::mem::size_of::<Self>() + 16;
        let pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        own + match pending.as_ref() {
            Some(Pending::Wire {
                shared,
                bytes,
                apply,
                ..
            }) => {
                16 + bytes.len()
                    + shared.retained_bound()
                    + apply.capacity() * std::mem::size_of::<Apply>()
                    + apply.iter().map(Apply::bytes).sum::<usize>()
            }
            Some(Pending::Mapped { source, apply }) => {
                source.retained_bound(owned)
                    + apply.capacity() * std::mem::size_of::<Apply>()
                    + apply.iter().map(Apply::bytes).sum::<usize>()
            }
            None => self.ready().map_or(0, owned),
        }
    }
    #[cfg(test)]
    pub(crate) fn pending_bytes(&self) -> usize {
        match self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            Some(Pending::Wire { bytes, apply, .. }) => {
                bytes.len() + 16 + apply.capacity() * std::mem::size_of::<Apply>()
            }
            Some(Pending::Mapped { source, apply }) => {
                source.pending_bytes() + apply.capacity() * std::mem::size_of::<Apply>()
            }
            None => 0,
        }
    }
}
impl Decoder<'_, '_> {
    pub(super) fn defer_component_field(
        &mut self,
        resolved: ResolvedType,
        array: bool,
        depth: u32,
        scope: DecodeScope,
    ) -> Result<Option<FigValue>, KiwiError> {
        let start = self.bb.index;
        self.skip_field(resolved, array, depth)?;
        let end = self.bb.index;
        if end - start < 128 {
            self.bb.index = start;
            return Ok(None);
        }
        if self.lazy_context.is_none() {
            self.lazy_context = Some(Arc::new(Shared {
                schema: self.schema.clone(),
                field_types: self.field_types.clone(),
                field_by_value: self.field_by_value.clone(),
                object_keys: self.object_keys.clone(),
                compact_guids: self.compact_guids.clone(),
                compact_affines: self.compact_affines.clone(),
                compact_floats: self.compact_floats.clone(),
            }));
        }
        Ok(Some(FigValue::Deferred(Deferred::new(Pending::Wire {
            shared: self.lazy_context.as_ref().unwrap().clone(),
            bytes: Arc::from(&self.bb.data[start..end]),
            spec: Spec {
                resolved,
                array,
                depth,
                scope,
                trailing: self.bb.array_remaining(),
            },
            apply: Vec::new(),
        }))))
    }
}

impl Shared {
    fn retained_bound(&self) -> usize {
        use std::mem::size_of;
        fn vec<T>(v: &Vec<T>) -> usize {
            v.capacity() * size_of::<T>()
        }
        fn arc_vec<T>(v: &Arc<Vec<T>>) -> usize {
            16 + size_of::<Vec<T>>() + vec(v)
        }
        let schema = vec(&self.schema.definitions)
            + self
                .schema
                .definitions
                .iter()
                .map(|d| {
                    d.name.capacity()
                        + vec(&d.fields)
                        + d.fields
                            .iter()
                            .map(|f| f.name.capacity() + f.type_name.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>();
        let types = arc_vec(&self.field_types) + self.field_types.iter().map(vec).sum::<usize>();
        let indices = arc_vec(&self.field_by_value)
            + self
                .field_by_value
                .iter()
                .map(|m| {
                    if m.capacity() == 0 {
                        0
                    } else {
                        m.capacity().next_power_of_two() * 2 * (size_of::<(u32, usize)>() + 1) + 16
                    }
                })
                .sum::<usize>();
        let keys = arc_vec(&self.object_keys)
            + self
                .object_keys
                .iter()
                .map(|v| vec(v) + v.iter().map(FigKey::retained_bytes).sum::<usize>())
                .sum::<usize>();
        size_of::<Self>()
            + 16
            + schema
            + types
            + indices
            + keys
            + arc_vec(&self.compact_guids)
            + arc_vec(&self.compact_affines)
            + arc_vec(&self.compact_floats)
    }
}

#[cfg(test)]
#[path = "deferred_tests.rs"]
mod tests;
