//! Built-in color themes. Ports slot-1 `runtime/themes.ts` (pure data).
//!
//! Every builtin shares one icon set (the TS reused `CATPPUCCIN_MOCHA.icons`
//! everywhere), so icons live in [`status_icon`] rather than per-theme.
//!
//! Deviation: the TS `PartialTheme` inline-object override is cut — the shared
//! settings model (`tt-config`) types `agentboard.theme` as a string name, so
//! partial inline themes were never representable on the Rust side.
//! `resolve_theme` takes the optional name and falls back to the default.

use crate::types::AgentStatus;

pub const DEFAULT_THEME: &str = "catppuccin-mocha";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    pub blue: &'static str,
    pub lavender: &'static str,
    pub pink: &'static str,
    pub mauve: &'static str,
    pub yellow: &'static str,
    pub green: &'static str,
    pub red: &'static str,
    pub peach: &'static str,
    pub teal: &'static str,
    pub sky: &'static str,
    pub text: &'static str,
    pub subtext0: &'static str,
    pub subtext1: &'static str,
    pub overlay0: &'static str,
    pub overlay1: &'static str,
    pub surface0: &'static str,
    pub surface1: &'static str,
    pub surface2: &'static str,
    pub base: &'static str,
    pub mantle: &'static str,
    pub crust: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusColors {
    pub idle: &'static str,
    pub running: &'static str,
    pub done: &'static str,
    pub error: &'static str,
    pub waiting: &'static str,
    pub question: &'static str,
    pub interrupted: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub palette: ThemePalette,
    pub status: StatusColors,
}

impl Theme {
    pub fn status_color(&self, status: AgentStatus) -> &'static str {
        match status {
            AgentStatus::Idle => self.status.idle,
            AgentStatus::Running => self.status.running,
            AgentStatus::Done => self.status.done,
            AgentStatus::Error => self.status.error,
            AgentStatus::Waiting => self.status.waiting,
            AgentStatus::Question => self.status.question,
            AgentStatus::Interrupted => self.status.interrupted,
        }
    }
}

/// Status icon (the single icon set all TS builtins shared).
pub fn status_icon(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "○",
        AgentStatus::Running => "●",
        AgentStatus::Done => "✓",
        AgentStatus::Error => "✗",
        AgentStatus::Waiting => "◉",
        AgentStatus::Question => "?",
        AgentStatus::Interrupted => "⚠",
    }
}

/// Resolve a theme by name; unknown or absent names fall back to the default.
pub fn resolve_theme(name: Option<&str>) -> &'static Theme {
    let key = name.unwrap_or(DEFAULT_THEME);
    BUILTIN_THEMES
        .iter()
        .find(|(n, _)| *n == key)
        .or_else(|| BUILTIN_THEMES.iter().find(|(n, _)| *n == DEFAULT_THEME))
        .map(|(_, t)| *t)
        .expect("default theme present")
}

// --- Builtin theme data (generated from runtime/themes.ts) ---

const CATPPUCCIN_MOCHA: Theme = Theme {
    palette: ThemePalette {
        blue: "#89b4fa",
        lavender: "#b4befe",
        pink: "#cba6f7",
        mauve: "#cba6f7",
        yellow: "#f9e2af",
        green: "#a6e3a1",
        red: "#f38ba8",
        peach: "#fab387",
        teal: "#94e2d5",
        sky: "#89dceb",
        text: "#cdd6f4",
        subtext0: "#a6adc8",
        subtext1: "#bac2de",
        overlay0: "#6c7086",
        overlay1: "#7f849c",
        surface0: "#313244",
        surface1: "#45475a",
        surface2: "#585b70",
        base: "#1e1e2e",
        mantle: "#181825",
        crust: "#11111b",
    },
    status: StatusColors {
        idle: "#585b70",
        running: "#f9e2af",
        done: "#a6e3a1",
        error: "#f38ba8",
        waiting: "#89b4fa",
        question: "#a6e3a1",
        interrupted: "#fab387",
    },
};

const CATPPUCCIN_LATTE: Theme = Theme {
    palette: ThemePalette {
        blue: "#1e66f5",
        lavender: "#7287fd",
        pink: "#ea76cb",
        mauve: "#8839ef",
        yellow: "#df8e1d",
        green: "#40a02b",
        red: "#d20f39",
        peach: "#fe640b",
        teal: "#179299",
        sky: "#04a5e5",
        text: "#4c4f69",
        subtext0: "#6c6f85",
        subtext1: "#5c5f77",
        overlay0: "#9ca0b0",
        overlay1: "#8c8fa1",
        surface0: "#ccd0da",
        surface1: "#bcc0cc",
        surface2: "#acb0be",
        base: "#eff1f5",
        mantle: "#e6e9ef",
        crust: "#dce0e8",
    },
    status: StatusColors {
        idle: "#acb0be",
        running: "#df8e1d",
        done: "#40a02b",
        error: "#d20f39",
        waiting: "#1e66f5",
        question: "#40a02b",
        interrupted: "#fe640b",
    },
};

const TOKYO_NIGHT: Theme = Theme {
    palette: ThemePalette {
        blue: "#7aa2f7",
        lavender: "#bb9af7",
        pink: "#bb9af7",
        mauve: "#bb9af7",
        yellow: "#e0af68",
        green: "#9ece6a",
        red: "#f7768e",
        peach: "#ff9e64",
        teal: "#73daca",
        sky: "#7dcfff",
        text: "#c0caf5",
        subtext0: "#a9b1d6",
        subtext1: "#9aa5ce",
        overlay0: "#565f89",
        overlay1: "#414868",
        surface0: "#24283b",
        surface1: "#292e42",
        surface2: "#343a52",
        base: "#1a1b26",
        mantle: "#16161e",
        crust: "#13131a",
    },
    status: StatusColors {
        idle: "#343a52",
        running: "#e0af68",
        done: "#9ece6a",
        error: "#f7768e",
        waiting: "#7aa2f7",
        question: "#9ece6a",
        interrupted: "#ff9e64",
    },
};

const GRUVBOX_DARK: Theme = Theme {
    palette: ThemePalette {
        blue: "#83a598",
        lavender: "#d3869b",
        pink: "#d3869b",
        mauve: "#d3869b",
        yellow: "#fabd2f",
        green: "#b8bb26",
        red: "#fb4934",
        peach: "#fe8019",
        teal: "#8ec07c",
        sky: "#83a598",
        text: "#ebdbb2",
        subtext0: "#d5c4a1",
        subtext1: "#bdae93",
        overlay0: "#665c54",
        overlay1: "#7c6f64",
        surface0: "#3c3836",
        surface1: "#504945",
        surface2: "#665c54",
        base: "#282828",
        mantle: "#1d2021",
        crust: "#1b1b1b",
    },
    status: StatusColors {
        idle: "#665c54",
        running: "#fabd2f",
        done: "#b8bb26",
        error: "#fb4934",
        waiting: "#83a598",
        question: "#b8bb26",
        interrupted: "#fe8019",
    },
};

const NORD: Theme = Theme {
    palette: ThemePalette {
        blue: "#81a1c1",
        lavender: "#b48ead",
        pink: "#b48ead",
        mauve: "#b48ead",
        yellow: "#ebcb8b",
        green: "#a3be8c",
        red: "#bf616a",
        peach: "#d08770",
        teal: "#8fbcbb",
        sky: "#88c0d0",
        text: "#eceff4",
        subtext0: "#d8dee9",
        subtext1: "#e5e9f0",
        overlay0: "#4c566a",
        overlay1: "#434c5e",
        surface0: "#3b4252",
        surface1: "#434c5e",
        surface2: "#4c566a",
        base: "#2e3440",
        mantle: "#272c36",
        crust: "#242933",
    },
    status: StatusColors {
        idle: "#4c566a",
        running: "#ebcb8b",
        done: "#a3be8c",
        error: "#bf616a",
        waiting: "#81a1c1",
        question: "#a3be8c",
        interrupted: "#d08770",
    },
};

const DRACULA: Theme = Theme {
    palette: ThemePalette {
        blue: "#8be9fd",
        lavender: "#bd93f9",
        pink: "#ff79c6",
        mauve: "#bd93f9",
        yellow: "#f1fa8c",
        green: "#50fa7b",
        red: "#ff5555",
        peach: "#ffb86c",
        teal: "#8be9fd",
        sky: "#8be9fd",
        text: "#f8f8f2",
        subtext0: "#bfbfbf",
        subtext1: "#6272a4",
        overlay0: "#6272a4",
        overlay1: "#565761",
        surface0: "#44475a",
        surface1: "#44475a",
        surface2: "#6272a4",
        base: "#282a36",
        mantle: "#21222c",
        crust: "#191a21",
    },
    status: StatusColors {
        idle: "#6272a4",
        running: "#f1fa8c",
        done: "#50fa7b",
        error: "#ff5555",
        waiting: "#8be9fd",
        question: "#50fa7b",
        interrupted: "#ffb86c",
    },
};

const CATPPUCCIN_FRAPPE: Theme = Theme {
    palette: ThemePalette {
        blue: "#8da4e2",
        lavender: "#babbf1",
        pink: "#f4b8e4",
        mauve: "#ca9ee6",
        yellow: "#e5c890",
        green: "#a6d189",
        red: "#e78284",
        peach: "#ef9f76",
        teal: "#81c8be",
        sky: "#99d1db",
        text: "#c6d0f5",
        subtext0: "#a5adce",
        subtext1: "#b5bfe2",
        overlay0: "#626880",
        overlay1: "#51576d",
        surface0: "#414559",
        surface1: "#51576d",
        surface2: "#626880",
        base: "#303446",
        mantle: "#292c3c",
        crust: "#232634",
    },
    status: StatusColors {
        idle: "#626880",
        running: "#e5c890",
        done: "#a6d189",
        error: "#e78284",
        waiting: "#8da4e2",
        question: "#a6d189",
        interrupted: "#ef9f76",
    },
};

const CATPPUCCIN_MACCHIATO: Theme = Theme {
    palette: ThemePalette {
        blue: "#8aadf4",
        lavender: "#b7bdf8",
        pink: "#f5bde6",
        mauve: "#c6a0f6",
        yellow: "#eed49f",
        green: "#a6da95",
        red: "#ed8796",
        peach: "#f5a97f",
        teal: "#8bd5ca",
        sky: "#91d7e3",
        text: "#cad3f5",
        subtext0: "#a5adcb",
        subtext1: "#b8c0e0",
        overlay0: "#5b6078",
        overlay1: "#494d64",
        surface0: "#363a4f",
        surface1: "#494d64",
        surface2: "#5b6078",
        base: "#24273a",
        mantle: "#1e2030",
        crust: "#181926",
    },
    status: StatusColors {
        idle: "#5b6078",
        running: "#eed49f",
        done: "#a6da95",
        error: "#ed8796",
        waiting: "#8aadf4",
        question: "#a6da95",
        interrupted: "#f5a97f",
    },
};

const GITHUB_DARK: Theme = Theme {
    palette: ThemePalette {
        blue: "#58a6ff",
        lavender: "#bc8cff",
        pink: "#bc8cff",
        mauve: "#bc8cff",
        yellow: "#e3b341",
        green: "#3fb950",
        red: "#f85149",
        peach: "#d29922",
        teal: "#39c5cf",
        sky: "#58a6ff",
        text: "#c9d1d9",
        subtext0: "#8b949e",
        subtext1: "#b1bac4",
        overlay0: "#484f58",
        overlay1: "#30363d",
        surface0: "#161b22",
        surface1: "#21262d",
        surface2: "#30363d",
        base: "#0d1117",
        mantle: "#010409",
        crust: "#010409",
    },
    status: StatusColors {
        idle: "#484f58",
        running: "#e3b341",
        done: "#3fb950",
        error: "#f85149",
        waiting: "#58a6ff",
        question: "#3fb950",
        interrupted: "#d29922",
    },
};

const ONE_DARK: Theme = Theme {
    palette: ThemePalette {
        blue: "#61afef",
        lavender: "#c678dd",
        pink: "#c678dd",
        mauve: "#c678dd",
        yellow: "#e5c07b",
        green: "#98c379",
        red: "#e06c75",
        peach: "#d19a66",
        teal: "#56b6c2",
        sky: "#61afef",
        text: "#abb2bf",
        subtext0: "#828997",
        subtext1: "#5c6370",
        overlay0: "#5c6370",
        overlay1: "#4b5263",
        surface0: "#3e4451",
        surface1: "#4b5263",
        surface2: "#5c6370",
        base: "#282c34",
        mantle: "#21252b",
        crust: "#1b1f27",
    },
    status: StatusColors {
        idle: "#5c6370",
        running: "#e5c07b",
        done: "#98c379",
        error: "#e06c75",
        waiting: "#61afef",
        question: "#98c379",
        interrupted: "#d19a66",
    },
};

const KANAGAWA: Theme = Theme {
    palette: ThemePalette {
        blue: "#7E9CD8",
        lavender: "#957FB8",
        pink: "#D27E99",
        mauve: "#957FB8",
        yellow: "#D7A657",
        green: "#98BB6C",
        red: "#E82424",
        peach: "#FFA066",
        teal: "#7AA89F",
        sky: "#7FB4CA",
        text: "#DCD7BA",
        subtext0: "#C8C093",
        subtext1: "#727169",
        overlay0: "#727169",
        overlay1: "#54546D",
        surface0: "#363646",
        surface1: "#54546D",
        surface2: "#727169",
        base: "#1F1F28",
        mantle: "#16161D",
        crust: "#131320",
    },
    status: StatusColors {
        idle: "#54546D",
        running: "#D7A657",
        done: "#98BB6C",
        error: "#E82424",
        waiting: "#7E9CD8",
        question: "#98BB6C",
        interrupted: "#FFA066",
    },
};

const EVERFOREST: Theme = Theme {
    palette: ThemePalette {
        blue: "#7fbbb3",
        lavender: "#d699b6",
        pink: "#d699b6",
        mauve: "#d699b6",
        yellow: "#dbbc7f",
        green: "#a7c080",
        red: "#e67e80",
        peach: "#e69875",
        teal: "#83c092",
        sky: "#7fbbb3",
        text: "#d3c6aa",
        subtext0: "#9da9a0",
        subtext1: "#7a8478",
        overlay0: "#7a8478",
        overlay1: "#859289",
        surface0: "#343f44",
        surface1: "#3d484d",
        surface2: "#475258",
        base: "#2d353b",
        mantle: "#272e33",
        crust: "#232a2e",
    },
    status: StatusColors {
        idle: "#7a8478",
        running: "#dbbc7f",
        done: "#a7c080",
        error: "#e67e80",
        waiting: "#7fbbb3",
        question: "#a7c080",
        interrupted: "#e69875",
    },
};

const MATERIAL: Theme = Theme {
    palette: ThemePalette {
        blue: "#82aaff",
        lavender: "#c792ea",
        pink: "#c792ea",
        mauve: "#c792ea",
        yellow: "#ffcb6b",
        green: "#c3e88d",
        red: "#f07178",
        peach: "#f78c6c",
        teal: "#89ddff",
        sky: "#82aaff",
        text: "#eeffff",
        subtext0: "#b0bec5",
        subtext1: "#546e7a",
        overlay0: "#546e7a",
        overlay1: "#37474f",
        surface0: "#37474f",
        surface1: "#455a64",
        surface2: "#546e7a",
        base: "#263238",
        mantle: "#1e272c",
        crust: "#192227",
    },
    status: StatusColors {
        idle: "#546e7a",
        running: "#ffcb6b",
        done: "#c3e88d",
        error: "#f07178",
        waiting: "#82aaff",
        question: "#c3e88d",
        interrupted: "#f78c6c",
    },
};

const COBALT2: Theme = Theme {
    palette: ThemePalette {
        blue: "#0088ff",
        lavender: "#9a5feb",
        pink: "#ff9d00",
        mauve: "#9a5feb",
        yellow: "#ffc600",
        green: "#9eff80",
        red: "#ff0088",
        peach: "#ff628c",
        teal: "#2affdf",
        sky: "#0088ff",
        text: "#ffffff",
        subtext0: "#adb7c9",
        subtext1: "#6688aa",
        overlay0: "#2d5a7b",
        overlay1: "#1f4662",
        surface0: "#1f4662",
        surface1: "#234b6b",
        surface2: "#2d5a7b",
        base: "#193549",
        mantle: "#122738",
        crust: "#0e1e2e",
    },
    status: StatusColors {
        idle: "#2d5a7b",
        running: "#ffc600",
        done: "#9eff80",
        error: "#ff0088",
        waiting: "#0088ff",
        question: "#9eff80",
        interrupted: "#ff628c",
    },
};

const FLEXOKI: Theme = Theme {
    palette: ThemePalette {
        blue: "#4385BE",
        lavender: "#8B7EC8",
        pink: "#CE5D97",
        mauve: "#8B7EC8",
        yellow: "#D0A215",
        green: "#879A39",
        red: "#D14D41",
        peach: "#DA702C",
        teal: "#3AA99F",
        sky: "#4385BE",
        text: "#CECDC3",
        subtext0: "#B7B5AC",
        subtext1: "#878580",
        overlay0: "#6F6E69",
        overlay1: "#575653",
        surface0: "#282726",
        surface1: "#343331",
        surface2: "#403E3C",
        base: "#100F0F",
        mantle: "#0D0D0C",
        crust: "#0A0A09",
    },
    status: StatusColors {
        idle: "#575653",
        running: "#D0A215",
        done: "#879A39",
        error: "#D14D41",
        waiting: "#4385BE",
        question: "#879A39",
        interrupted: "#DA702C",
    },
};

const AYU: Theme = Theme {
    palette: ThemePalette {
        blue: "#59C2FF",
        lavender: "#D2A6FF",
        pink: "#F07178",
        mauve: "#D2A6FF",
        yellow: "#E6B450",
        green: "#7FD962",
        red: "#D95757",
        peach: "#FF8F40",
        teal: "#95E6CB",
        sky: "#39BAE6",
        text: "#BFBDB6",
        subtext0: "#ACB6BF",
        subtext1: "#565B66",
        overlay0: "#565B66",
        overlay1: "#6C7380",
        surface0: "#0D1017",
        surface1: "#0F131A",
        surface2: "#11151C",
        base: "#0B0E14",
        mantle: "#090C10",
        crust: "#070A0E",
    },
    status: StatusColors {
        idle: "#565B66",
        running: "#E6B450",
        done: "#7FD962",
        error: "#D95757",
        waiting: "#59C2FF",
        question: "#7FD962",
        interrupted: "#FF8F40",
    },
};

const AURA: Theme = Theme {
    palette: ThemePalette {
        blue: "#82e2ff",
        lavender: "#a277ff",
        pink: "#f694ff",
        mauve: "#a277ff",
        yellow: "#ffca85",
        green: "#9dff65",
        red: "#ff6767",
        peach: "#ffca85",
        teal: "#61ffca",
        sky: "#82e2ff",
        text: "#edecee",
        subtext0: "#bdbdbd",
        subtext1: "#6d6d6d",
        overlay0: "#6d6d6d",
        overlay1: "#2d2d2d",
        surface0: "#1a1a24",
        surface1: "#1f1f2b",
        surface2: "#2d2d2d",
        base: "#15141b",
        mantle: "#110f17",
        crust: "#0f0f0f",
    },
    status: StatusColors {
        idle: "#6d6d6d",
        running: "#ffca85",
        done: "#61ffca",
        error: "#ff6767",
        waiting: "#a277ff",
        question: "#61ffca",
        interrupted: "#ffca85",
    },
};

const MATRIX: Theme = Theme {
    palette: ThemePalette {
        blue: "#30b3ff",
        lavender: "#c770ff",
        pink: "#c770ff",
        mauve: "#c770ff",
        yellow: "#e6ff57",
        green: "#62ff94",
        red: "#ff4b4b",
        peach: "#ffa83d",
        teal: "#24f6d9",
        sky: "#30b3ff",
        text: "#62ff94",
        subtext0: "#8ca391",
        subtext1: "#4a6b55",
        overlay0: "#2e4a37",
        overlay1: "#1e2a1b",
        surface0: "#141c12",
        surface1: "#182218",
        surface2: "#1e2a1b",
        base: "#0a0e0a",
        mantle: "#070a07",
        crust: "#050705",
    },
    status: StatusColors {
        idle: "#2e4a37",
        running: "#e6ff57",
        done: "#62ff94",
        error: "#ff4b4b",
        waiting: "#30b3ff",
        question: "#62ff94",
        interrupted: "#ffa83d",
    },
};

const TRANSPARENT: Theme = Theme {
    palette: ThemePalette {
        blue: "#89b4fa",
        lavender: "#b4befe",
        pink: "#cba6f7",
        mauve: "#cba6f7",
        yellow: "#f9e2af",
        green: "#a6e3a1",
        red: "#f38ba8",
        peach: "#fab387",
        teal: "#94e2d5",
        sky: "#89dceb",
        text: "#cdd6f4",
        subtext0: "#a6adc8",
        subtext1: "#bac2de",
        overlay0: "#6c7086",
        overlay1: "#7f849c",
        surface0: "#313244",
        surface1: "#45475a",
        surface2: "#585b70",
        base: "transparent",
        mantle: "transparent",
        crust: "transparent",
    },
    status: CATPPUCCIN_MOCHA.status,
};

pub const BUILTIN_THEMES: &[(&str, &Theme)] = &[
    ("catppuccin-mocha", &CATPPUCCIN_MOCHA),
    ("catppuccin-latte", &CATPPUCCIN_LATTE),
    ("catppuccin-frappe", &CATPPUCCIN_FRAPPE),
    ("catppuccin-macchiato", &CATPPUCCIN_MACCHIATO),
    ("tokyo-night", &TOKYO_NIGHT),
    ("gruvbox-dark", &GRUVBOX_DARK),
    ("nord", &NORD),
    ("dracula", &DRACULA),
    ("github-dark", &GITHUB_DARK),
    ("one-dark", &ONE_DARK),
    ("kanagawa", &KANAGAWA),
    ("everforest", &EVERFOREST),
    ("material", &MATERIAL),
    ("cobalt2", &COBALT2),
    ("flexoki", &FLEXOKI),
    ("ayu", &AYU),
    ("aura", &AURA),
    ("matrix", &MATRIX),
    ("transparent", &TRANSPARENT),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_all_19_builtins_and_default() {
        assert_eq!(BUILTIN_THEMES.len(), 19);
        assert!(BUILTIN_THEMES.iter().any(|(n, _)| *n == DEFAULT_THEME));
    }

    #[test]
    fn resolve_falls_back_to_default() {
        let default = resolve_theme(None);
        assert_eq!(default.palette.base, "#1e1e2e");
        assert_eq!(resolve_theme(Some("no-such-theme")).palette.base, "#1e1e2e");
        assert_eq!(resolve_theme(Some("nord")).palette.base, "#2e3440");
    }

    #[test]
    fn spot_check_against_ts_source() {
        let latte = resolve_theme(Some("catppuccin-latte"));
        assert_eq!(latte.status.running, "#df8e1d");
        assert_eq!(latte.palette.crust, "#dce0e8");

        let transparent = resolve_theme(Some("transparent"));
        assert_eq!(transparent.palette.base, "transparent");
        // transparent reuses mocha's status colors.
        assert_eq!(transparent.status, resolve_theme(None).status);

        let matrix = resolve_theme(Some("matrix"));
        assert_eq!(matrix.palette.text, "#62ff94");
    }

    #[test]
    fn status_colors_and_icons_cover_every_status() {
        let theme = resolve_theme(None);
        for s in [
            AgentStatus::Idle,
            AgentStatus::Running,
            AgentStatus::Done,
            AgentStatus::Error,
            AgentStatus::Waiting,
            AgentStatus::Question,
            AgentStatus::Interrupted,
        ] {
            assert!(!theme.status_color(s).is_empty());
            assert!(!status_icon(s).is_empty());
        }
        assert_eq!(status_icon(AgentStatus::Done), "✓");
    }
}
