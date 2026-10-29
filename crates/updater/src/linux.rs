use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Installer {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
}

pub fn installer(package: &Path, exists: impl Fn(&Path) -> bool) -> Option<Installer> {
    for (program, option) in [
        ("/usr/bin/gdebi-gtk", None),
        ("/usr/bin/plasma-discover", Some("--local-filename")),
        ("/usr/bin/gnome-software", Some("--local-filename")),
    ] {
        if exists(Path::new(program)) {
            let mut arguments = Vec::new();
            if let Some(option) = option {
                arguments.push(OsString::from(option));
            }
            arguments.push(package.as_os_str().to_owned());
            return Some(Installer {
                program: program.into(),
                arguments,
            });
        }
    }
    None
}

pub fn manual_command(package: &Path) -> String {
    // POSIX single quoting also covers spaces, apostrophes and shell substitutions in cache paths.
    format!(
        "sudo apt install '{}'",
        package.to_string_lossy().replace('\'', "'\\''")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_installers_get_package_as_one_argument_and_missing_tools_fall_back() {
        let package = Path::new("/home/Alice Smith/cache/new.deb");
        let launch = installer(package, |path| {
            path == Path::new("/usr/bin/plasma-discover")
        })
        .unwrap();
        assert_eq!(
            launch.arguments,
            vec![
                OsString::from("--local-filename"),
                package.as_os_str().to_owned()
            ]
        );
        assert!(installer(package, |_| false).is_none());
        assert_eq!(
            manual_command(Path::new("/home/a'b/new.deb")),
            "sudo apt install '/home/a'\\''b/new.deb'"
        );
    }
}
