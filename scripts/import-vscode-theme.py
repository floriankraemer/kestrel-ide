#!/usr/bin/env python3
"""Regenerate `third_party/github-vscode-theme/themes/*.toml` from the
vendored upstream JSON in `third_party/github-vscode-theme/upstream/`.

Unlike `scripts/import-material-icons.py`, this script does not reimplement
the VS Code theme mapping itself: `crates/color-theme`'s `parse_vscode_json`
is the one true mapping (it's what this IDE's own Import Theme feature
runs on a user-supplied theme file at runtime), so re-deriving colours from
scratch here in Python would let a second mapping drift from it. Instead
this script is a thin loop that shells out to a small Rust example,
`crates/color-theme/examples/import_vscode_theme.rs`, which calls the real
`parse_vscode_json` and writes the result back out as TOML.

The nine `(json file, id, label, appearance)` entries below are static:
`third_party/github-vscode-theme/plugin.toml`'s nine
`[[contributes.color-themes]]` entries, hand-maintained for the same
reason (they don't change shape between upstream releases). `id`/`label`
and `appearance` are passed explicitly rather than left to the parser's
own heuristics because none of these nine JSON files set a `type` key —
VS Code gets dark/light from the extension manifest's `uiTheme`, which
this script reads off instead — and the two colourblind variants' upstream
display labels carry a "(Beta)" suffix their JSON `name` field lacks.

This does NOT download anything: `upstream/*.json` must already be
vendored (see `third_party/github-vscode-theme/README.md` for how to
refresh it from a newer release).

Usage: from inside the container (`make shell`), then
    python3 scripts/import-vscode-theme.py
"""

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PACK_DIR = REPO / "third_party" / "github-vscode-theme"
UPSTREAM_DIR = PACK_DIR / "upstream"
THEMES_DIR = PACK_DIR / "themes"

# (upstream json stem, id, label, appearance) — mirrors plugin.toml's
# nine [[contributes.color-themes]] entries.
THEMES = [
    ("light-default", "github-light-default", "GitHub Light Default", "light"),
    ("light-high-contrast", "github-light-high-contrast", "GitHub Light High Contrast", "light"),
    ("light-colorblind", "github-light-colorblind", "GitHub Light Colorblind (Beta)", "light"),
    ("dark-default", "github-dark-default", "GitHub Dark Default", "dark"),
    ("dark-high-contrast", "github-dark-high-contrast", "GitHub Dark High Contrast", "dark"),
    ("dark-colorblind", "github-dark-colorblind", "GitHub Dark Colorblind (Beta)", "dark"),
    ("dark-dimmed", "github-dark-dimmed", "GitHub Dark Dimmed", "dark"),
    ("light", "github-light", "GitHub Light", "light"),
    ("dark", "github-dark", "GitHub Dark", "dark"),
]


def main() -> None:
    THEMES_DIR.mkdir(parents=True, exist_ok=True)
    for stem, theme_id, label, appearance in THEMES:
        src = UPSTREAM_DIR / f"{stem}.json"
        dst = THEMES_DIR / f"{stem}.toml"
        if not src.exists():
            sys.exit(f"missing vendored upstream file: {src}")
        subprocess.run(
            [
                "cargo",
                "run",
                "-q",
                "-p",
                "color-theme",
                "--example",
                "import_vscode_theme",
                "--",
                str(src),
                str(dst),
                theme_id,
                label,
                appearance,
            ],
            check=True,
            cwd=REPO,
        )
        print(f"wrote {dst.relative_to(REPO)}")


if __name__ == "__main__":
    main()
