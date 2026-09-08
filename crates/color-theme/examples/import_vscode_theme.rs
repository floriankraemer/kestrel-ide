//! Converts one VS Code theme JSON file into this crate's native TOML
//! shape, through the real runtime mapping (`color_theme::parse_vscode_json`)
//! rather than a second, hand-rolled one — see `scripts/import-vscode-theme.py`,
//! which shells out to this example once per upstream file.
//!
//! `parse_vscode_json` derives `id`/`label` from the JSON's own `name` field
//! and `appearance` from its `type` field. The nine `github-vscode-theme`
//! files this exists for set neither consistently (`type` is absent from
//! all of them — VS Code gets dark/light from the *extension manifest*'s
//! `uiTheme`, not the theme file) nor do their bare names carry the
//! "(Beta)" suffix the upstream manifest's display labels use for the two
//! colorblind variants. So this binary takes the correct `id`/`label`/
//! `appearance` as arguments and overrides the parsed result with them,
//! rather than trusting a heuristic that these particular files don't
//! support — the colour and syntax mapping itself, the part that actually
//! needs to match the runtime, is untouched.
//!
//! Usage: `cargo run -p color-theme --example import_vscode_theme -- \
//!   <input.json> <output.toml> <id> <label> <dark|light>`

use std::collections::BTreeMap;
use std::fs;
use std::process::ExitCode;

use color_theme::{Appearance, ColorTheme, Rgba};

fn hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
}

fn to_toml(theme: &ColorTheme) -> String {
    let c = &theme.chrome;
    let s = &theme.semantic;
    let d = &theme.diff;
    let t = &theme.terminal;

    // BTreeMap for deterministic, sorted output — a re-run against the
    // same upstream file must produce a byte-identical TOML, so a real
    // upstream change is what shows up in the diff.
    let syntax: BTreeMap<_, _> = theme.syntax.iter().collect();
    let mut syntax_toml = String::new();
    for (name, style) in syntax {
        syntax_toml.push_str(&format!(
            "\n[syntax.\"{name}\"]\nfg = \"{}\"\nbold = {}\nitalic = {}\nunderline = {}\n",
            hex(style.fg),
            style.bold,
            style.italic,
            style.underline,
        ));
    }

    format!(
        r#"id = "{id}"
label = "{label}"
appearance = "{appearance}"

[chrome]
canvas = "{canvas}"
surface = "{surface}"
surface2 = "{surface2}"
raised = "{raised}"
border = "{border}"
text = "{text}"
text_dim = "{text_dim}"
accent = "{accent}"
accent_ink = "{accent_ink}"
selection = "{selection}"
status_bar = "{status_bar}"

[semantic]
error = "{error}"
warning = "{warning}"
info = "{info}"
ok = "{ok}"
muted = "{muted}"

[diff]
added_line = "{added_line}"
added_inline = "{added_inline}"
added_marker = "{added_marker}"
modified_line = "{modified_line}"
modified_inline = "{modified_inline}"
modified_marker = "{modified_marker}"
deleted_line = "{deleted_line}"
deleted_inline = "{deleted_inline}"
deleted_marker = "{deleted_marker}"

[terminal]
black = "{black}"
red = "{red}"
green = "{green}"
yellow = "{yellow}"
blue = "{blue}"
magenta = "{magenta}"
cyan = "{cyan}"
white = "{white}"
bright_black = "{bright_black}"
bright_red = "{bright_red}"
bright_green = "{bright_green}"
bright_yellow = "{bright_yellow}"
bright_blue = "{bright_blue}"
bright_magenta = "{bright_magenta}"
bright_cyan = "{bright_cyan}"
bright_white = "{bright_white}"
background = "{background}"
foreground = "{foreground}"
cursor = "{cursor}"
selection = "{terminal_selection}"
{syntax_toml}"#,
        id = theme.id,
        label = theme.label,
        appearance = match theme.appearance {
            Appearance::Dark => "dark",
            Appearance::Light => "light",
        },
        canvas = hex(c.canvas),
        surface = hex(c.surface),
        surface2 = hex(c.surface2),
        raised = hex(c.raised),
        border = hex(c.border),
        text = hex(c.text),
        text_dim = hex(c.text_dim),
        accent = hex(c.accent),
        accent_ink = hex(c.accent_ink),
        selection = hex(c.selection),
        status_bar = hex(c.status_bar),
        error = hex(s.error),
        warning = hex(s.warning),
        info = hex(s.info),
        ok = hex(s.ok),
        muted = hex(s.muted),
        added_line = hex(d.added_line),
        added_inline = hex(d.added_inline),
        added_marker = hex(d.added_marker),
        modified_line = hex(d.modified_line),
        modified_inline = hex(d.modified_inline),
        modified_marker = hex(d.modified_marker),
        deleted_line = hex(d.deleted_line),
        deleted_inline = hex(d.deleted_inline),
        deleted_marker = hex(d.deleted_marker),
        black = hex(t.black),
        red = hex(t.red),
        green = hex(t.green),
        yellow = hex(t.yellow),
        blue = hex(t.blue),
        magenta = hex(t.magenta),
        cyan = hex(t.cyan),
        white = hex(t.white),
        bright_black = hex(t.bright_black),
        bright_red = hex(t.bright_red),
        bright_green = hex(t.bright_green),
        bright_yellow = hex(t.bright_yellow),
        bright_blue = hex(t.bright_blue),
        bright_magenta = hex(t.bright_magenta),
        bright_cyan = hex(t.bright_cyan),
        bright_white = hex(t.bright_white),
        background = hex(t.background),
        foreground = hex(t.foreground),
        cursor = hex(t.cursor),
        terminal_selection = hex(t.selection),
        syntax_toml = syntax_toml,
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output, id, label, appearance] = &args[..] else {
        eprintln!(
            "usage: import_vscode_theme <input.json> <output.toml> <id> <label> <dark|light>"
        );
        return ExitCode::FAILURE;
    };

    let appearance = match appearance.as_str() {
        "dark" => Appearance::Dark,
        "light" => Appearance::Light,
        other => {
            eprintln!("appearance must be \"dark\" or \"light\", got \"{other}\"");
            return ExitCode::FAILURE;
        }
    };

    let json = match fs::read_to_string(input) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("reading {input}: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut theme = match color_theme::parse_vscode_json(&json) {
        Ok(theme) => theme,
        Err(err) => {
            eprintln!("parsing {input}: {err}");
            return ExitCode::FAILURE;
        }
    };
    theme.id = id.clone();
    theme.label = label.clone();
    theme.appearance = appearance;

    if let Err(err) = fs::write(output, to_toml(&theme)) {
        eprintln!("writing {output}: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
