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

Files are embedded in the application. An executable-adjacent `terminal-colors/`
directory can override them with the same filenames; development also searches the
repository directory. Missing/invalid overrides fall back to embedded data.
