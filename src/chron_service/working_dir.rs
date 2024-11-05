use dirs::home_dir;
use std::path::PathBuf;

// Expand an initial tilde component in the directory
pub fn expand_working_dir(dir: &PathBuf) -> PathBuf {
    if let Some(home) = home_dir() {
        let mut components = dir.components();
        if let Some(first_component) = components.next() {
            let mut expanded_dir = home;
            if first_component.as_os_str() == "~" {
                for component in components {
                    expanded_dir.push(component);
                }
                return expanded_dir;
            }
        };
    }
    dir.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    #[test]
    fn test_initial_solitary_tilde() {
        assert_eq!(
            expand_working_dir(&PathBuf::from("~/project")),
            dirs::home_dir().unwrap().join("project")
        );
    }

    #[test]
    fn test_non_solitary_tilde() {
        assert_eq!(
            expand_working_dir(&PathBuf::from("~a/project")),
            PathBuf::from("~a/project")
        );
    }

    #[test]
    fn test_non_initial_tilde() {
        assert_eq!(
            expand_working_dir(&PathBuf::from("/~/project")),
            PathBuf::from("/~/project")
        );
    }
}
