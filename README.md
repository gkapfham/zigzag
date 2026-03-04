# Pathfinder

A [Zellij](https://zellij.dev) plugin for navigating opened tabs and panes.

Select a tab: 

![Tabs](./assets/tab.png)

Select pane from any tab in the session:
 
![Pane](./assets/pane.png)


## Enhanced Features

### Enhanced Fuzzy Matching

The fuzzy search is case-insensitive and tolerant of mistypes. For example,
typing `ageM` will match `Manager` because all the characters are present
in the candidate. This works across all views (Tabs, Panes, and Sessions).

### Cross-Tab Pane Search

The Panes Selector shows **all panes across all tabs** in the current session,
not just the panes in the active tab. Each pane is displayed with its tab name
in brackets (e.g., `my-pane [Editor]`). Selecting a pane automatically switches
to the correct tab and focuses it.


## Key Bindings


| Key            | Action                           | Mode     |
| ---------------| ---------------------------------|----------|
| Up/Down key    | move up/down list                | tab/pane |
| PageUp         | move top of list                 | tab only |
| Esc/Ctrl + c   | quit                             | tab/pane |
| Tab            | switch mode                      | tab/pane |
| Left/Right key | move left/right cursor in prompt | tab/pane |




## Should I use this plugin? 

This is plugin is a mixed of functionalities from [session-manager](https://github.com/zellij-org/zellij/tree/main/default-plugins/session-manager) and [room](https://github.com/rvcas/room), you should look at those first.


## Requirements

Zellij version `v0.41.1` or later.


### Zellij Plugin Permission 

| Permission               | Why                                         |
| -------------------------|---------------------------------------------|
| `ReadApplicationState`   | Subscribe to Pane and tab events            |
| `ChangeApplicationState` | Setting plugin pane name, creating new tabs |



## Install

### Download WASM Binary


* Download `pathfinder.wasm` binary from [release page](https://github.com/vdbulcke/pathfinder/releases).
* Verify binary signature with cosign (see instruction bellow)
* copy binary to zellij plugin dir: 
     - `mv target/wasm32-wasip1/release/pathfinder.wasm ~/.config/zellij/plugins/`

#### Validate Signature With Cosign

Make sure you have `cosign` installed locally (see [Cosign Install](https://docs.sigstore.dev/cosign/installation/)).

Then you can use the `./verify_signature.sh` in this repo: 

```bash
./verify_signature.sh PATH_TO_DOWNLOADED_ARCHIVE TAG_VERSION
```
for example
```bash
$ ./verify_signature.sh ~/Downloads/pathfinder.wasm v0.1.0

Checking Signature for version: v0.1.0
Verified OK

```


### Build from source

> WARNING: requires to have rust installed and wasm `rustup target add wasm32-wasip1`

* `git clone git@github.com:vdbulcke/pathfinder.git`
* `cd pathfinder`
* `cargo build --release`
* `mv target/wasm32-wasip1/release/pathfinder.wasm ~/.config/zellij/plugins/`



## Loading plugin 

### Via Zellij Config

```kdl
shared_except "locked" {
    bind "Ctrl y" {
        LaunchOrFocusPlugin "file:~/.config/zellij/plugins/pathfinder.wasm" {
            floating true
       
        }
    }
}
```
