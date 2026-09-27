use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportError {
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

fn import_of(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix("#import")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map(str::trim)
}

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
        out.push_str(&text);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
