//! The engine's WGSL module system: a line `#import <module>` in any shader
//! source — engine or pack — is replaced by that module's text, once per
//! composed source however many pieces import it.
//!
//! Every module is GENERATED from the Rust definition it mirrors, so a shader
//! can no longer carry a stale hand copy:
//!
//! - `petramond::frame` — the frame [`Uniforms`](crate::uniforms::Uniforms)
//!   struct and `PETRAMOND_FRAME_ABI`, from `uniforms::UNIFORM_FIELDS`;
//! - `petramond::shader_params` — the pack shaders' named parameter slots;
//! - `petramond::vertex` — the packed vertex words' lane decoders, UV modes,
//!   normal codes, block-light and transition decodes, from
//!   `petramond_mesh::vertex`;
//! - `petramond::uv_rects` — the tile uv-rect table binding.
//!
//! Pack sky and environment shaders bind the same frame buffer, so for them
//! this is the ABI: importing `petramond::frame` instead of copying the struct
//! keeps a pack correct when a field is added.

use std::borrow::Cow;

/// An `#import` naming no module this engine provides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportError {
    /// 1-based line of the import in the source that spelled it.
    pub line: usize,
    pub module: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "line {}: unknown shader import `{}` (available: {})",
            self.line,
            self.module,
            MODULES.join(", ")
        )
    }
}

impl std::error::Error for ImportError {}

/// Every importable module, by the name a shader spells.
const MODULES: [&str; 4] = [
    "petramond::frame",
    "petramond::shader_params",
    "petramond::vertex",
    "petramond::uv_rects",
];

fn module_source(name: &str) -> Option<String> {
    Some(match name {
        "petramond::frame" => crate::uniforms::frame_wgsl(),
        "petramond::shader_params" => crate::uniforms::shader_params_wgsl(),
        "petramond::vertex" => petramond_mesh::vertex::wgsl::layout(),
        "petramond::uv_rects" => crate::uniforms::UV_RECTS_WGSL.to_owned(),
        _ => return None,
    })
}

/// The module a line imports, if it is an import line.
fn import_of(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix("#import")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map(str::trim)
}

/// Resolve every `#import` in `source`. A source without imports comes back
/// borrowed, untouched.
pub(crate) fn compose(source: &str) -> Result<Cow<'_, str>, ImportError> {
    if !source.lines().any(|line| import_of(line).is_some()) {
        return Ok(Cow::Borrowed(source));
    }
    let mut out = String::with_capacity(source.len() * 2);
    let mut included = Vec::new();
    expand(source, &mut out, &mut included)?;
    Ok(Cow::Owned(out))
}

fn expand<'a>(
    source: &'a str,
    out: &mut String,
    included: &mut Vec<&'a str>,
) -> Result<(), ImportError> {
    for (index, line) in source.lines().enumerate() {
        let Some(module) = import_of(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        if included.contains(&module) {
            continue;
        }
        let text = module_source(module).ok_or_else(|| ImportError {
            line: index + 1,
            module: module.to_owned(),
        })?;
        included.push(module);
        // Generated modules are leaves: none imports another.
        out.push_str(&text);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
