//! Shared colored icons for terminal tabs and the remote host status item.
use gpui::Img;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SystemIcon {
    #[default]
    Bash,
    PowerShell,
    Linux,
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    OpenSuse,
}

impl SystemIcon {
    pub(crate) fn shell(program: &str) -> Self {
        let name = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase();
        match name.strip_suffix(".exe").unwrap_or(&name) {
            "powershell" | "pwsh" => Self::PowerShell,
            _ => Self::Bash,
        }
    }

    pub(crate) fn remote(id: &str, like: &str, kernel: &str) -> Self {
        fn distribution(id: &str) -> Option<SystemIcon> {
            match id.to_ascii_lowercase().as_str() {
                "ubuntu" => Some(SystemIcon::Ubuntu),
                "debian" => Some(SystemIcon::Debian),
                "fedora" => Some(SystemIcon::Fedora),
                "arch" | "archlinux" => Some(SystemIcon::Arch),
                "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" | "suse" | "sles" => {
                    Some(SystemIcon::OpenSuse)
                }
                _ => None,
            }
        }
        distribution(id)
            .or_else(|| like.split_whitespace().find_map(distribution))
            .unwrap_or(if kernel.eq_ignore_ascii_case("linux") {
                Self::Linux
            } else {
                Self::Bash
            })
    }

    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Bash => "icons/systems/gnubash.svg",
            Self::PowerShell => "icons/systems/powershell.svg",
            Self::Linux => "icons/systems/linux.svg",
            Self::Ubuntu => "icons/systems/ubuntu.svg",
            Self::Debian => "icons/systems/debian.svg",
            Self::Fedora => "icons/systems/fedora.svg",
            Self::Arch => "icons/systems/archlinux.svg",
            Self::OpenSuse => "icons/systems/openSUSE.svg",
        }
    }

    pub(crate) fn element(self) -> Img {
        crate::assets::colored_icon(self.path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_shells_use_the_executable_not_the_title() {
        for program in [
            "pwsh",
            "/opt/microsoft/powershell/7/pwsh",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ] {
            assert_eq!(SystemIcon::shell(program), SystemIcon::PowerShell);
        }
        for program in ["/bin/bash", "/bin/zsh", "unknown", ""] {
            assert_eq!(SystemIcon::shell(program), SystemIcon::Bash);
        }
    }
    #[test]
    fn remote_distributions_prefer_id_then_family_then_kernel() {
        assert_eq!(
            SystemIcon::remote("ubuntu", "debian", "Linux"),
            SystemIcon::Ubuntu
        );
        assert_eq!(
            SystemIcon::remote("linuxmint", "ubuntu debian", "Linux"),
            SystemIcon::Ubuntu
        );
        assert_eq!(
            SystemIcon::remote("manjaro", "arch", "Linux"),
            SystemIcon::Arch
        );
        assert_eq!(
            SystemIcon::remote("opensuse-tumbleweed", "suse", "Linux"),
            SystemIcon::OpenSuse
        );
        assert_eq!(SystemIcon::remote("alpine", "", "Linux"), SystemIcon::Linux);
        assert_eq!(SystemIcon::remote("", "", ""), SystemIcon::Bash);
        assert_eq!(
            SystemIcon::remote("freebsd", "", "FreeBSD"),
            SystemIcon::Bash
        );
    }
    #[test]
    fn all_system_icons_are_embedded() {
        for icon in [
            SystemIcon::Bash,
            SystemIcon::PowerShell,
            SystemIcon::Linux,
            SystemIcon::Ubuntu,
            SystemIcon::Debian,
            SystemIcon::Fedora,
            SystemIcon::Arch,
            SystemIcon::OpenSuse,
        ] {
            assert!(
                crate::assets::Assets::get(icon.path()).is_some(),
                "{}",
                icon.path()
            );
        }
    }
}
