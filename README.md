# Zigzag

A [Zellij](https://zellij.dev) plugin for navigating
opened tabs and panes. With `zigzag` you can effortlessly
`zig` and `zag` between open tabs and panes in a Zellij
session.

## About

This is a fork of the
[pathfinder](https://github.com/vdbulcke/pathfinder)
plugin for Zellij. I created this fork to add small
enhancements that are helpful for my workflow. I am
maintaining this fork for my own use and do not currently
plan to release it as an official package.

## Enhanced Features

### Enhanced Fuzzy Matching

The fuzzy search is case-insensitive and tolerant of
mistypes. For example, typing `ageM` will match
`Manager` because all the characters are present in the
candidate. This works across all views (Tabs, Panes,
and Sessions).

### Cross-Tab Pane Search

The Panes Selector shows **all panes across all tabs**
in the current session, not just the panes in the
active tab. Each pane is displayed with its tab name
in brackets (e.g., `my-pane [Editor]`). Selecting a
pane automatically switches to the correct tab and
focuses it.

## Key Bindings

| Key | Action | Mode |
| ---------------| ---------------------------------|--------------------|
| Up/Down key | move up/down list | tab/pane/session |
| PageUp | move top of list | tab only |
| Esc/Ctrl + c | quit | tab/pane/session |
| Tab | switch mode | tab/pane/session |
| Left/Right key | move left/right cursor in prompt | tab/pane/session |

## Requirements

Zellij version `v0.41.1` or later.

### Zellij Plugin Permission

| Permission | Why |
| -------------------------|---------------------------------------------|
| `ReadApplicationState` | Subscribe to Pane and tab events |
| `ChangeApplicationState` | Setting plugin pane name, creating new tabs |

## Build from Source

> **Note:** This requires Rust and the WASM target to
> be installed.

```bash
# Install the WASM target if you haven't already
rustup target add wasm32-wasip1

# Clone the repository
git clone https://github.com/gkapfham/zigzag.git
cd zigzag

# Build the plugin
cargo build --release

# Copy the compiled plugin to your Zellij plugins directory
mv target/wasm32-wasip1/release/zigzag.wasm ~/.config/zellij/plugins/
```

## Loading the Plugin

### Via Zellij Config

```kdl
shared_except "locked" {
    bind "Ctrl y" {
        LaunchOrFocusPlugin "file:~/.config/zellij/plugins/zigzag.wasm" {
            floating true
        }
    }
}
```

## About the Development

I used an LLM-based coding agent to add features to
this plugin as an opportunity to learn about Rust
programming and WebAssembly, technologies I am not
familiar with. This was a learning exercise for me.

## About the Author

Learn more about me at
[gregorykapfhammer.com](https://www.gregorykapfhammer.com/).
