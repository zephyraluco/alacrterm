use std::env;
use std::path::PathBuf;

pub fn find_in_env(filename: &str) -> Option<String> {
    env::var_os("PATH")
        .and_then(|paths| {
            env::split_paths(&paths).find_map(|mut dir| {
                dir.push(filename);
                if dir.is_file() {
                    Some(filename.to_string())
                } else {
                    None
                }
            })
        })
}