fn names_a_file(base: &str) -> bool {
    const EXACT: &[&str] = &[
        "i",
        "y",
        "n",
        "f",
        "map",
        "attach",
        "dump_attachment",
        "lavfi",
        "vf",
        "af",
        "pass",
        "passlogfile",
        "report",
        "progress",
        "sdp_file",
    ];
    EXACT.contains(&base)
        || base.starts_with("filter")
        || base.ends_with("pre")
        || base.starts_with("vstats")
        || base.starts_with("stats_")
}

pub fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub fn option_problem(name: &str, value: &str) -> Option<String> {
    let name_ok = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_:-".contains(&b));
    let base = name.split(':').next().unwrap_or_default();
    if !name_ok || names_a_file(base) {
        return Some(format!("option '{name}' is not one a mod may set"));
    }
    let value_ok = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_+.-".contains(&b))
        && (!value.starts_with('-') || value.parse::<f64>().is_ok());
    if !value_ok {
        return Some(format!(
            "option '{name}' has a value '{value}' a mod may not pass"
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mod_can_name_no_option_that_touches_a_file() {
        for name in [
            "i",
            "y",
            "n",
            "f",
            "map",
            "attach",
            "dump_attachment",
            "lavfi",
            "vf",
            "af",
            "pass",
            "passlogfile",
            "report",
            "progress",
            "sdp_file",
            "filter",
            "filter_complex",
            "filter_script",
            "vpre",
            "fpre",
            "vstats_file",
            "stats_period",
        ] {
            for spelling in [name.to_owned(), format!("{name}:v"), format!("{name}:a:0")] {
                assert!(
                    option_problem(&spelling, "1").is_some(),
                    "-{spelling} reached the encoder"
                );
            }
        }
        for (name, value) in [
            ("b:v", "/etc/passwd"),
            ("preset", "-y"),
            ("-y", "1"),
            ("Crf", "1"),
            ("x264-params", "stats=x"),
        ] {
            assert!(option_problem(name, value).is_some(), "-{name} {value}");
        }
        for (name, value) in [
            ("crf", "18"),
            ("movflags", "+faststart"),
            ("itsoffset", "-1.5"),
        ] {
            assert_eq!(option_problem(name, value), None, "-{name} {value}");
        }
        assert!(is_token("libx264") && is_token("mp4"));
        assert!(!is_token("mp4 -i") && !is_token("") && !is_token("../x"));
    }
}
