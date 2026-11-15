//! Node-free document metadata capture, compacted to changed fields/keys.
use super::header_map::{exact, MapPatch};
use jian_ops_schema::PenDocument;
use serde::Serialize;
use std::{collections::BTreeMap, mem};

#[derive(Debug)]
pub(super) struct PendingHeader {
    version: String,
    name: Option<String>,
    format_version: Option<String>,
    responsive: Option<bool>,
    id: Option<String>,
    app: Option<jian_ops_schema::app::AppConfig>,
    routes: Option<jian_ops_schema::routes::RoutesConfig>,
    lifecycle: Option<jian_ops_schema::lifecycle::AppLifecycleHooks>,
    logic_modules: Option<Vec<jian_ops_schema::logic_module::LogicModuleRef>>,
    design_md: Option<jian_ops_schema::design_md::DesignMdSpec>,
    conversion: Option<jian_ops_schema::conversion::ConversionSpec>,
    motion: Option<jian_ops_schema::motion::MotionPreference>,
    variables: Option<BTreeMap<String, jian_ops_schema::variable::VariableDefinition>>,
    themes: Option<BTreeMap<String, Vec<String>>>,
    state: Option<BTreeMap<String, jian_ops_schema::state::StateEntry>>,
}
#[derive(Debug, Clone, PartialEq)]
pub(super) struct HeaderPatch {
    version: Option<Box<String>>,
    name: Option<Box<Option<String>>>,
    format_version: Option<Box<Option<String>>>,
    responsive: Option<Box<Option<bool>>>,
    id: Option<Box<Option<String>>>,
    app: Option<Box<Option<jian_ops_schema::app::AppConfig>>>,
    routes: Option<Box<Option<jian_ops_schema::routes::RoutesConfig>>>,
    lifecycle: Option<Box<Option<jian_ops_schema::lifecycle::AppLifecycleHooks>>>,
    logic_modules: Option<Box<Option<Vec<jian_ops_schema::logic_module::LogicModuleRef>>>>,
    design_md: Option<Box<Option<jian_ops_schema::design_md::DesignMdSpec>>>,
    conversion: Option<Box<Option<jian_ops_schema::conversion::ConversionSpec>>>,
    motion: Option<Box<Option<jian_ops_schema::motion::MotionPreference>>>,
    variables: Option<MapPatch<jian_ops_schema::variable::VariableDefinition>>,
    themes: Option<MapPatch<Vec<String>>>,
    state: Option<MapPatch<jian_ops_schema::state::StateEntry>>,
}
// Exhaustive destructuring makes new document fields require a conscious
// scope classification; page/root trees never enter this header capture.
impl PendingHeader {
    pub(super) fn capture(doc: &PenDocument) -> Self {
        let PenDocument {
            version,
            name,
            format_version,
            responsive,
            id,
            app,
            routes,
            lifecycle,
            logic_modules,
            design_md,
            conversion,
            motion,
            variables,
            themes,
            state,
            children: _,
            pages: _,
        } = doc;
        Self {
            version: version.clone(),
            name: name.clone(),
            format_version: format_version.clone(),
            responsive: responsive.clone(),
            id: id.clone(),
            app: app.clone(),
            routes: routes.clone(),
            lifecycle: lifecycle.clone(),
            logic_modules: logic_modules.clone(),
            design_md: design_md.clone(),
            conversion: conversion.clone(),
            motion: motion.clone(),
            variables: variables.clone(),
            themes: themes.clone(),
            state: state.clone(),
        }
    }
    pub(super) fn matches(&self, doc: &PenDocument) -> bool {
        exact(&self.version, &doc.version)
            && exact(&self.name, &doc.name)
            && exact(&self.format_version, &doc.format_version)
            && exact(&self.responsive, &doc.responsive)
            && exact(&self.id, &doc.id)
            && exact(&self.app, &doc.app)
            && exact(&self.routes, &doc.routes)
            && exact(&self.lifecycle, &doc.lifecycle)
            && exact(&self.logic_modules, &doc.logic_modules)
            && exact(&self.design_md, &doc.design_md)
            && exact(&self.conversion, &doc.conversion)
            && exact(&self.motion, &doc.motion)
            && exact(&self.variables, &doc.variables)
            && exact(&self.themes, &doc.themes)
            && exact(&self.state, &doc.state)
    }
    pub(super) fn restore(self, doc: &mut PenDocument) {
        doc.version = self.version;
        doc.name = self.name;
        doc.format_version = self.format_version;
        doc.responsive = self.responsive;
        doc.id = self.id;
        doc.app = self.app;
        doc.routes = self.routes;
        doc.lifecycle = self.lifecycle;
        doc.logic_modules = self.logic_modules;
        doc.design_md = self.design_md;
        doc.conversion = self.conversion;
        doc.motion = self.motion;
        doc.variables = self.variables;
        doc.themes = self.themes;
        doc.state = self.state;
    }
    pub(super) fn compact(self, doc: &PenDocument) -> HeaderPatch {
        HeaderPatch {
            version: (!exact(&self.version, &doc.version)).then(|| Box::new(self.version)),
            name: (!exact(&self.name, &doc.name)).then(|| Box::new(self.name)),
            format_version: (!exact(&self.format_version, &doc.format_version))
                .then(|| Box::new(self.format_version)),
            responsive: (!exact(&self.responsive, &doc.responsive))
                .then(|| Box::new(self.responsive)),
            id: (!exact(&self.id, &doc.id)).then(|| Box::new(self.id)),
            app: (!exact(&self.app, &doc.app)).then(|| Box::new(self.app)),
            routes: (!exact(&self.routes, &doc.routes)).then(|| Box::new(self.routes)),
            lifecycle: (!exact(&self.lifecycle, &doc.lifecycle)).then(|| Box::new(self.lifecycle)),
            logic_modules: (!exact(&self.logic_modules, &doc.logic_modules))
                .then(|| Box::new(self.logic_modules)),
            design_md: (!exact(&self.design_md, &doc.design_md)).then(|| Box::new(self.design_md)),
            conversion: (!exact(&self.conversion, &doc.conversion))
                .then(|| Box::new(self.conversion)),
            motion: (!exact(&self.motion, &doc.motion)).then(|| Box::new(self.motion)),
            variables: MapPatch::compact(self.variables, &doc.variables),
            themes: MapPatch::compact(self.themes, &doc.themes),
            state: MapPatch::compact(self.state, &doc.state),
        }
    }
}
impl HeaderPatch {
    pub(super) fn validate(&self, doc: &PenDocument) -> bool {
        self.variables
            .as_ref()
            .is_none_or(|patch| patch.validate(&doc.variables))
            && self
                .themes
                .as_ref()
                .is_none_or(|patch| patch.validate(&doc.themes))
            && self
                .state
                .as_ref()
                .is_none_or(|patch| patch.validate(&doc.state))
    }
    pub(super) fn exchange(&mut self, doc: &mut PenDocument) {
        if let Some(old) = &mut self.version {
            mem::swap(old.as_mut(), &mut doc.version);
        }
        if let Some(old) = &mut self.name {
            mem::swap(old.as_mut(), &mut doc.name);
        }
        if let Some(old) = &mut self.format_version {
            mem::swap(old.as_mut(), &mut doc.format_version);
        }
        if let Some(old) = &mut self.responsive {
            mem::swap(old.as_mut(), &mut doc.responsive);
        }
        if let Some(old) = &mut self.id {
            mem::swap(old.as_mut(), &mut doc.id);
        }
        if let Some(old) = &mut self.app {
            mem::swap(old.as_mut(), &mut doc.app);
        }
        if let Some(old) = &mut self.routes {
            mem::swap(old.as_mut(), &mut doc.routes);
        }
        if let Some(old) = &mut self.lifecycle {
            mem::swap(old.as_mut(), &mut doc.lifecycle);
        }
        if let Some(old) = &mut self.logic_modules {
            mem::swap(old.as_mut(), &mut doc.logic_modules);
        }
        if let Some(old) = &mut self.design_md {
            mem::swap(old.as_mut(), &mut doc.design_md);
        }
        if let Some(old) = &mut self.conversion {
            mem::swap(old.as_mut(), &mut doc.conversion);
        }
        if let Some(old) = &mut self.motion {
            mem::swap(old.as_mut(), &mut doc.motion);
        }
        if let Some(patch) = &mut self.variables {
            patch.exchange(&mut doc.variables);
        }
        if let Some(patch) = &mut self.themes {
            patch.exchange(&mut doc.themes);
        }
        if let Some(patch) = &mut self.state {
            patch.exchange(&mut doc.state);
        }
    }
    pub(super) fn storage_bytes(&self) -> usize {
        [
            self.version.as_ref().map_or(0, |value| {
                mem::size_of::<String>().saturating_add(retained_bytes(value.as_ref()))
            }),
            self.name.as_ref().map_or(0, |value| {
                mem::size_of::<Option<String>>().saturating_add(retained_bytes(value.as_ref()))
            }),
            self.format_version.as_ref().map_or(0, |value| {
                mem::size_of::<Option<String>>().saturating_add(retained_bytes(value.as_ref()))
            }),
            self.responsive.as_ref().map_or(0, |value| {
                mem::size_of::<Option<bool>>().saturating_add(retained_bytes(value.as_ref()))
            }),
            self.id.as_ref().map_or(0, |value| {
                mem::size_of::<Option<String>>().saturating_add(retained_bytes(value.as_ref()))
            }),
            self.app.as_ref().map_or(0, |value| {
                mem::size_of::<Option<jian_ops_schema::app::AppConfig>>()
                    .saturating_add(retained_bytes(value.as_ref()))
            }),
            self.routes.as_ref().map_or(0, |value| {
                mem::size_of::<Option<jian_ops_schema::routes::RoutesConfig>>()
                    .saturating_add(retained_bytes(value.as_ref()))
            }),
            self.lifecycle.as_ref().map_or(0, |value| {
                mem::size_of::<Option<jian_ops_schema::lifecycle::AppLifecycleHooks>>()
                    .saturating_add(retained_bytes(value.as_ref()))
            }),
            self.logic_modules.as_ref().map_or(0, |value| {
                mem::size_of::<Option<Vec<jian_ops_schema::logic_module::LogicModuleRef>>>()
                    .saturating_add(retained_bytes(value.as_ref()))
            }),
            self.design_md.as_ref().map_or(0, |value| {
                mem::size_of::<Option<jian_ops_schema::design_md::DesignMdSpec>>()
                    .saturating_add(retained_bytes(value.as_ref()))
            }),
            self.conversion.as_ref().map_or(0, |value| {
                mem::size_of::<Option<jian_ops_schema::conversion::ConversionSpec>>()
                    .saturating_add(retained_bytes(value.as_ref()))
            }),
            self.motion.as_ref().map_or(0, |value| {
                mem::size_of::<Option<jian_ops_schema::motion::MotionPreference>>()
                    .saturating_add(retained_bytes(value.as_ref()))
            }),
            self.variables.as_ref().map_or(0, MapPatch::storage_bytes),
            self.themes.as_ref().map_or(0, MapPatch::storage_bytes),
            self.state.as_ref().map_or(0, MapPatch::storage_bytes),
        ]
        .into_iter()
        .fold(0usize, usize::saturating_add)
    }
}
// Values were cloned into the pending header, so owned strings/Vec buffers
// have no caller-controlled spare capacity. This deliberately conservative
// serialized structural charge includes nested metadata containers, while
// shared node/component storage remains separately identity-accounted.
pub(super) fn retained_bytes<T: Serialize + ?Sized>(value: &T) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Count(0);
    let result =
        jian_ops_schema::image_table::without_save_scope(|| serde_json::to_writer(&mut out, value));
    if result.is_err() {
        usize::MAX
    } else {
        out.0.saturating_mul(32).saturating_add(1024)
    }
}
