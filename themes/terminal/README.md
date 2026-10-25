# Terminal color schemes

Six dark palettes are available: Catppuccin Mocha, Tokyo Night, One Dark, Nord,
Gruvbox Dark and Solarized Dark. Each JSON contains background/foreground,
normal/bright/dim ANSI colors, selection, search and link colors. Dim ANSI colors
are derived by blending each normal color 66% with the background 34%.

Palette references:
- https://github.com/catppuccin/catppuccin
- https://github.com/folke/tokyonight.nvim
- https://github.com/atom/one-dark-syntax
- https://www.nordtheme.com/docs/colors-and-palettes
- https://github.com/morhetz/gruvbox
- https://ethanschoonover.com/solarized/

Files are embedded in the application. An executable-adjacent `themes/terminal/`
directory can override them with the same filenames. The app watches the first
existing `themes/terminal/` directory beside the executable or in the repository.
Missing overrides use embedded data. Invalid edits retain the last valid version;
invalid files on first load use the embedded palette where available.

Choose a palette in **设置 → 终端 → 终端配色**. Changes apply immediately to
existing tabs and split panes, and new sessions use the same palette. The chosen
ID is saved as `color_scheme` in `config/terminal.json` and restored on restart.
A named palette takes precedence over that file's custom color keys. Choosing
**默认／自定义** restores those custom colors (or renderer defaults if absent).
Font, completion and unknown configuration fields are preserved.

Edit and save a JSON file to update the selected palette automatically, after a
short debounce. New JSON files appear in the dropdown; their filenames are stable
IDs and their `name` fields are display labels. Deleting an override restores its
embedded palette. Deleting a custom-only palette temporarily uses default/custom
colors, retaining the selected ID so recreating the file restores it automatically.
Hot reload never writes `config/terminal.json` and never changes font/input options.
UI themes remain independent of terminal palettes.
