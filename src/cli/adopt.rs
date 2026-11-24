//! The value of `--adopt`: a comma-separated list of image names.

/// Normalized image names: lower case, always with the `.exe` suffix (`NAME`
/// and `NAME.exe` are the same), duplicates dropped. No paths, wildcards or
/// empty elements; at most 16 names of at most 64 characters.
pub(super) fn parse_adopt_names(value: &str) -> Result<Vec<String>, String> {
    let invalid = || "invalid --adopt value".to_string();
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() > 16 {
        return Err(invalid());
    }
    let mut names = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty()
            || part.chars().count() > 64
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?'))
        {
            return Err(invalid());
        }
        let lower = part.to_lowercase();
        let name = if lower.ends_with(".exe") {
            lower
        } else {
            format!("{lower}.exe")
        };
        if !names.iter().any(|existing| existing == &name) {
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::parse_adopt_names;

    #[test]
    fn names_are_normalized() {
        assert_eq!(
            parse_adopt_names("Microsoft Edge,Tool Name.exe").unwrap(),
            vec!["microsoft edge.exe", "tool name.exe"]
        );
        assert_eq!(
            parse_adopt_names("MSBuild,msbuild.EXE,node").unwrap(),
            vec!["msbuild.exe", "node.exe"]
        );
    }

    #[test]
    fn invalid_lists_are_refused() {
        let long = "a".repeat(65);
        let many = "a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q";
        for value in [
            "",
            ",node",
            "node,",
            "node,,tool",
            "../x",
            "a\\b",
            "*",
            "?",
            "name:stream.exe",
            long.as_str(),
            many,
        ] {
            assert_eq!(
                parse_adopt_names(value),
                Err("invalid --adopt value".into()),
                "{value}"
            );
        }
    }
}
