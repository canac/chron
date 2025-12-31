use crate::chronfile::env::Env;
use std::path::PathBuf;

// Expand an initial tilde component in the directory
pub fn expand_tilde(dir: &PathBuf, env: &Env) -> PathBuf {
    let mut components = dir.components();
    if let Some(first_component) = components.next() {
        let mut expanded_dir = env.home_dir.clone();
        if first_component.as_os_str() == "~" {
            for component in components {
                expanded_dir.push(component);
            }
            return expanded_dir;
        }
    }
    dir.to_owned()
}

#[cfg(test)]
mod tests {
    use crate::chronfile::env::Env;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_initial_solitary_tilde() {
        assert_eq!(
            expand_tilde(&PathBuf::from("~/project"), &Env::mock()),
            PathBuf::from("/Users/user/project"),
        );
    }

    #[test]
    fn test_non_solitary_tilde() {
        assert_eq!(
            expand_tilde(&PathBuf::from("~a/project"), &Env::mock()),
            PathBuf::from("~a/project"),
        );
    }

    #[test]
    fn test_non_initial_tilde() {
        assert_eq!(
            expand_tilde(&PathBuf::from("/~/project"), &Env::mock()),
            PathBuf::from("/~/project"),
        );
    }
}
