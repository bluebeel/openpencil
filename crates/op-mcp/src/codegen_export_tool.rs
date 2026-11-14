//! `codegen_export` — deterministic code export, no model involved.
//!
//! Runs one of the pure `op-codegen` generators over a node selection and
//! returns the source text. `react-tailwind` (the default) additionally
//! returns its theme files (`app/globals.css`, `tailwind.config.ts`) as
//! separate entries so an agent or the `op codegen:export` CLI can write
//! them straight to disk. Unlike the `codegen_plan` → `codegen_assemble`
//! chunk workflow, the output is byte-identical for the same document.
//!
//! Target nodes, first match wins: `nodeIds` (anywhere in the document),
//! `pageId` (that page's roots), the live selection, the active page.

use std::collections::{BTreeMap, HashMap};

use jian_ops_schema::node::PenNode;
use jian_ops_schema::PenDocument;
use op_codegen::{
    referenced_components, Codegen, Compose, CssVariables, Flutter, Html, React, ReactNative,
    ReactTailwind, Svelte, SwiftUi, Vue,
};
use op_editor_core::codegen::Framework;
use op_editor_core::{EditorState, PenNodeExt};
use serde_json::{json, Map, Value};

use super::{McpTool, ToolErrorCode, ToolOutcome};

/// Wire token of the variable-table-only target.
const CSS_VARIABLES: &str = "css-variables";
/// Target used when the caller names none.
const DEFAULT_TARGET: &str = "react-tailwind";

pub struct CodegenExport {
    snapshot: Option<CodegenInput>,
    prepared: Option<ToolOutcome>,
}

struct CodegenInput {
    doc: PenDocument,
    active_page: usize,
    selection: Vec<String>,
}

struct ExportContext<'a> {
    doc: &'a PenDocument,
    active_page: usize,
    selection: Vec<&'a str>,
}

/// Every wire token `codegen_export` accepts, in catalog order.
pub fn codegen_export_targets() -> Vec<&'static str> {
    Framework::ALL
        .iter()
        .map(|framework| framework.as_wire())
        .chain(std::iter::once(CSS_VARIABLES))
        .collect()
}

pub fn codegen_export_snapshot(state: &EditorState) -> CodegenExport {
    CodegenExport {
        snapshot: Some(CodegenInput {
            doc: state.doc.clone(),
            active_page: state.ui.active_page_index,
            selection: state
                .selection
                .set
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
        }),
        prepared: None,
    }
}

/// Prepare this exact request from borrowed source trees. Generators own only
/// the selected roots and document metadata, never the full source document.
pub fn codegen_export_snapshot_for_call(
    state: &EditorState,
    args: &BTreeMap<String, String>,
) -> CodegenExport {
    let context = ExportContext {
        doc: &state.doc,
        active_page: state.ui.active_page_index,
        selection: state.selection.set.iter().map(|id| id.as_str()).collect(),
    };
    CodegenExport {
        snapshot: None,
        prepared: Some(context.result(args)),
    }
}

impl McpTool for CodegenExport {
    fn name(&self) -> &str {
        "codegen_export"
    }

    fn call_once(mut self: Box<Self>, args: &BTreeMap<String, String>) -> ToolOutcome {
        match self.prepared.take() {
            Some(outcome) => outcome,
            None => self.call(args),
        }
    }

    fn call(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        if let Some(outcome) = &self.prepared {
            return outcome.clone();
        }
        let Some(input) = &self.snapshot else {
            return ToolOutcome::Err(ToolErrorCode::Internal, "missing codegen snapshot".into());
        };
        ExportContext {
            doc: &input.doc,
            active_page: input.active_page,
            selection: input.selection.iter().map(String::as_str).collect(),
        }
        .result(args)
    }
}

impl ExportContext<'_> {
    fn result(&self, args: &BTreeMap<String, String>) -> ToolOutcome {
        let target = args
            .get("framework")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_TARGET);
        let framework = Framework::from_wire(target);
        if framework.is_none() && target != CSS_VARIABLES {
            return ToolOutcome::Err(
                ToolErrorCode::InvalidArgument,
                format!(
                    "unknown framework {target:?}; expected one of: {}",
                    codegen_export_targets().join(", ")
                ),
            );
        }
        let roots = match self.target_nodes(args) {
            Ok(roots) => roots,
            Err(message) => return ToolOutcome::Err(ToolErrorCode::ToolFailed, message),
        };
        // CSS variables consume metadata only, while retaining the same target
        // validation/nodeCount as every other format.
        let scoped = scoped_document(self.doc, if framework.is_some() { &roots } else { &[] });

        let mut files = Map::new();
        let code = match framework {
            Some(Framework::ReactTailwind) => {
                let components = referenced_components(&roots, self.doc);
                let generated = ReactTailwind.generate_files_with_components(&scoped, &components);
                files.insert("component.tsx".into(), json!(generated.component));
                files.insert("app/globals.css".into(), json!(generated.globals_css));
                files.insert(
                    "tailwind.config.ts".into(),
                    json!(generated.tailwind_config),
                );
                generated.to_single_file()
            }
            Some(Framework::React) => React.generate(&scoped),
            Some(Framework::Vue) => Vue.generate(&scoped),
            Some(Framework::Svelte) => Svelte.generate(&scoped),
            Some(Framework::Html) => Html.generate(&scoped),
            Some(Framework::Flutter) => Flutter.generate(&scoped),
            Some(Framework::SwiftUi) => SwiftUi.generate(&scoped),
            Some(Framework::Compose) => Compose.generate(&scoped),
            Some(Framework::ReactNative) => ReactNative.generate(&scoped),
            None => CssVariables.generate(&scoped),
        };
        if files.is_empty() {
            files.insert(format!("component.{}", extension(framework)), json!(code));
        }
        let out = json!({
            "framework": target,
            "nodeCount": roots.len(),
            "code": code,
            "files": Value::Object(files),
        });
        ToolOutcome::OkJson(out.to_string())
    }

    fn pages(&self) -> Vec<(&str, &[PenNode])> {
        match self.doc.pages.as_deref() {
            Some(pages) if !pages.is_empty() => pages
                .iter()
                .map(|page| (page.id.as_str(), page.children.as_slice()))
                .collect(),
            _ => vec![("0", self.doc.children.as_slice())],
        }
    }

    fn find_requested<S: AsRef<str>>(&self, ids: &[S]) -> Vec<Option<&PenNode>> {
        let mut found = HashMap::new();
        for node in super::requested_nodes::find_in_forests(
            self.pages().into_iter().map(|(_, roots)| roots),
            ids,
        ) {
            // Ordered forests preserve first-page/DFS identity and requested
            // duplicates without rescanning later pages for resolved targets.
            found.entry(node.id_str()).or_insert(node);
        }
        ids.iter()
            .map(|id| found.get(id.as_ref()).copied())
            .collect()
    }

    fn target_nodes(&self, args: &BTreeMap<String, String>) -> Result<Vec<&PenNode>, String> {
        if let Some(raw) = args.get("nodeIds").filter(|raw| !raw.trim().is_empty()) {
            let ids = parse_ids(raw)?;
            let mut nodes = Vec::with_capacity(ids.len());
            for (id, node) in ids.iter().zip(self.find_requested(&ids)) {
                nodes.push(node.ok_or_else(|| format!("node not found: {id}"))?);
            }
            return Ok(nodes);
        }
        let pages = self.pages();
        if let Some(page_id) = args.get("pageId").filter(|p| !p.is_empty()) {
            let (_, roots) = pages
                .iter()
                .find(|(id, _)| id == page_id)
                .ok_or_else(|| format!("page not found: {page_id}"))?;
            return non_empty(roots.iter().collect());
        }
        let selected: Vec<&PenNode> = self
            .find_requested(&self.selection)
            .into_iter()
            .flatten()
            .collect();
        if !selected.is_empty() {
            return Ok(selected);
        }
        let index = self.active_page.min(pages.len().saturating_sub(1));
        non_empty(pages[index].1.iter().collect())
    }
}

/// Clone the complete typed header without touching any unselected forests.
/// This explicit struct initializer also requires new schema fields to be
/// handled at compile time; optional/empty header values are not normalized.
fn scoped_document(doc: &PenDocument, roots: &[&PenNode]) -> PenDocument {
    PenDocument {
        version: doc.version.clone(),
        name: doc.name.clone(),
        themes: doc.themes.clone(),
        variables: doc.variables.clone(),
        pages: None,
        children: roots.iter().map(|node| (*node).clone()).collect(),
        format_version: doc.format_version.clone(),
        responsive: doc.responsive,
        id: doc.id.clone(),
        app: doc.app.clone(),
        routes: doc.routes.clone(),
        state: doc.state.clone(),
        lifecycle: doc.lifecycle.clone(),
        logic_modules: doc.logic_modules.clone(),
        design_md: doc.design_md.clone(),
        conversion: doc.conversion.clone(),
        motion: doc.motion,
    }
}

fn non_empty(nodes: Vec<&PenNode>) -> Result<Vec<&PenNode>, String> {
    if nodes.is_empty() {
        Err("nothing to export: the target page is empty".into())
    } else {
        Ok(nodes)
    }
}

/// `nodeIds` as a JSON string array or a comma-separated list.
fn parse_ids(raw: &str) -> Result<Vec<String>, String> {
    let raw = raw.trim();
    if raw.starts_with('[') {
        return serde_json::from_str::<Vec<String>>(raw)
            .map_err(|e| format!("nodeIds must be a JSON string array: {e}"));
    }
    Ok(raw
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect())
}

fn extension(framework: Option<Framework>) -> &'static str {
    match framework {
        Some(Framework::React | Framework::ReactTailwind | Framework::ReactNative) => "tsx",
        Some(Framework::Vue) => "vue",
        Some(Framework::Svelte) => "svelte",
        Some(Framework::Html) => "html",
        Some(Framework::Flutter) => "dart",
        Some(Framework::SwiftUi) => "swift",
        Some(Framework::Compose) => "kt",
        None => "css",
    }
}
