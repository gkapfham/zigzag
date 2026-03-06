// =============================================================================
// Zigzag — A Zellij plugin for fuzzy-finding and navigating tabs, panes,
// and sessions. Note that this is a fork of the pathfinder plugin:
// https://github.com/vdbulcke/pathfinder that is not currently intended
// for release as an official plugin. It represents a minimal extension
// of the pathfinder codebase to support cross-tab pane searching and
// session searching, and serves as a playground for experimenting with the
// Rust programming language and the use of cargo to create WebAssembly
// plugins for Zellij.
//
// HOW THE ZELLIJ PLUGIN SYSTEM USES THIS FILE:
//
//   Zellij plugins are WebAssembly (WASM) modules. This file is compiled to
//   a `.wasm` binary via `cargo build --release` (the build target is set to
//   `wasm32-wasip1` in `.cargo/config.toml`). When Zellij loads the plugin,
//   it looks for a type registered with the `register_plugin!` macro (see the
//   bottom of this file) and calls the three trait methods defined by the
//   `ZellijPlugin` trait:
//
//     1. `load()`   — Called once when the plugin starts. Used to request
//                     permissions, subscribe to events, and set the pane name.
//     2. `update()` — Called each time an event arrives (tab changes, pane
//                     changes, key presses, session updates). Returns `true`
//                     if the UI should be redrawn.
//     3. `render()` — Called to draw the plugin UI. Receives the available
//                     rows and columns so the output can fit the pane.
//
// STATIC STRUCTURE OF THIS FILE:
//
//   1. Imports          — External crates and the Zellij plugin prelude.
//   2. View enum        — The three modes the user can switch between.
//   3. FlatPane struct  — A flattened pane entry for cross-tab pane search.
//   4. State struct     — All plugin state (current view, search input,
//                         matched items, fuzzy matcher, etc.).
//   5. State impl       — Helper methods on State, organized into groups:
//        a. Fuzzy matching   — enhanced_fuzzy_match(), is_match()
//        b. Pane rebuilding  — rebuild_all_panes()
//        c. Key handling     — handle_key_event()
//        d. Tab helpers      — get_focused_tab(), fuzzy_find_tab(), seek_tab(),
//                              move_down_tab(), move_up_tab()
//        e. Session helpers  — fuzzy_find_session(), move_down_session(),
//                              move_up_session()
//        f. Pane helpers     — fuzzy_find_pane(), select_pane_at_index(),
//                              move_down_pane(), move_up_pane()
//        g. Input helpers    — remove_input_at_index(), insert_input_at_index()
//        h. UI helpers       — print_prompt(), print_non_empty_input_prompt(),
//                              close(), change_mode()
//   6. ZellijPlugin impl — The three trait methods (load, update, render)
//                          that Zellij calls to run the plugin.
//   7. Constants & helpers — Color constants and the color_bold() function.
//
// DYNAMIC BEHAVIOR (what happens at runtime):
//
//   When the user presses a keybinding (e.g., Alt+y) to launch Zigzag:
//
//     1. Zellij loads the WASM and calls `load()`. The plugin requests
//        permissions and subscribes to tab/pane/session/key events.
//     2. Zellij immediately sends `TabUpdate`, `PaneUpdate`, and
//        `SessionUpdate` events. The plugin stores this data in State.
//     3. Zellij calls `render()` to draw the initial UI — showing the
//        Tab Selector view with all tabs listed.
//     4. As the user types, Zellij sends `Key` events. The plugin updates
//        the search input and re-runs fuzzy matching to filter/rank items.
//     5. Arrow keys move the selection highlight up/down through matches.
//        The Tab key cycles between Tab → Pane → Session views.
//     6. When the user presses Enter, the plugin closes itself and tells
//        Zellij to switch to the selected tab/pane/session.
// =============================================================================

use ansi_term::{Colour::Fixed, Style};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use owo_colors::OwoColorize;

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

/// The three main views the user can switch between by pressing the Tab key.
/// Each view presents a different list of items to search through.
enum View {
    /// Search and switch between Zellij sessions
    Session,
    /// Search and switch between tabs in the current session
    Tab,
    /// Search and switch between panes across ALL tabs in the current session
    Pane,
}

/// A flattened representation of a single pane from any tab in the session.
///
/// Zellij provides panes grouped by tab (via `PaneManifest`), but we want the
/// user to search across ALL panes at once. This struct stores the information
/// needed to display a pane in the list and to navigate to it when selected.
#[derive(Clone)]
struct FlatPane {
    /// The pane's unique numeric ID (used by Zellij's `focus_terminal_pane()`)
    pane_id: u32,
    /// The display title of the pane (e.g., the running command or custom name)
    title: String,
    /// The 0-indexed position of the tab this pane belongs to
    tab_position: usize,
    /// The human-readable name of the tab this pane belongs to
    tab_name: String,
}

/// The main plugin state. Holds everything needed to run the fuzzy finder UI:
/// the current view, the search input, the lists of tabs/panes/sessions,
/// the current match selections, and the fuzzy matcher.
struct State {
    /// User-provided plugin configuration from the Zellij config file
    userspace_configuration: BTreeMap<String, String>,

    /// Which view (Tab, Pane, or Session) is currently active
    current_view: View,
    /// The 0-indexed position of the currently focused (active) tab
    focus_tab_pos: usize,
    /// The index of the currently highlighted item in the displayed list
    result_index: usize,
    /// All tab information received from Zellij's TabUpdate event
    tab_infos: Vec<TabInfo>,
    /// Raw pane manifest from Zellij (panes grouped by tab position).
    /// The keys are 0-indexed tab positions, the values are lists of panes.
    pane_manifest: PaneManifest,
    /// The current search/filter input string typed by the user
    input: String,
    /// The cursor position within the input string (for Left/Right navigation)
    input_cusror_index: usize,
    /// The index of the best-matching tab (into the tab_infos vector)
    tab_match: Option<usize>,
    /// The name of the best-matching session
    session_match: Option<String>,
    /// The pane ID of the best-matching pane
    pane_match: Option<u32>,
    /// The display title of the best-matching pane
    pane_title_match: String,
    /// The tab position that the best-matching pane belongs to (so we can
    /// switch to that tab before focusing the pane)
    pane_tab_position: Option<usize>,
    /// A flattened, sorted list of ALL non-plugin panes across ALL tabs.
    /// Rebuilt whenever the pane manifest or tab info changes.
    all_panes: Vec<FlatPane>,
    /// All session names in the current Zellij instance
    sessions: Vec<String>,
    /// The fuzzy matcher instance, configured for case-insensitive matching
    fz_matcher: SkimMatcherV2,
}

/// Creates the default (initial) plugin state. The fuzzy matcher is set to
/// case-insensitive mode so that searches like "m" will match "Manager".
impl Default for State {
    fn default() -> Self {
        Self {
            userspace_configuration: BTreeMap::default(),
            current_view: View::Tab,

            focus_tab_pos: 0,
            result_index: 0,
            tab_infos: Vec::default(),
            pane_manifest: PaneManifest::default(),
            input: String::default(),
            input_cusror_index: 0,
            tab_match: None,
            session_match: None,
            pane_match: None,
            pane_title_match: String::default(),
            pane_tab_position: None,
            all_panes: Vec::default(),
            sessions: Vec::default(),
            fz_matcher: SkimMatcherV2::default().ignore_case(),
        }
    }
}

// =============================================================================
// State implementation — all the helper methods that power the plugin logic.
// =============================================================================
impl State {
    // -------------------------------------------------------------------------
    // Fuzzy matching helpers
    // -------------------------------------------------------------------------

    /// Perform an enhanced fuzzy match of a query against a candidate string.
    ///
    /// This uses a two-pass approach:
    ///   1. First, try the standard skim fuzzy matcher (case-insensitive). This
    ///      looks for the query characters as a subsequence of the candidate
    ///      (e.g., "mgr" matches "Manager"). If it finds a match, it returns
    ///      a score indicating how good the match is.
    ///   2. If the standard matcher fails, fall back to a "bag of characters"
    ///      check: verify that every character in the query exists somewhere in
    ///      the candidate (in any order, case-insensitive). Each character is
    ///      consumed once to prevent double-counting.
    ///
    /// The fallback handles cases like "ageM" matching "Manager" — the letters
    /// a, g, e, m are all present in "manager" even though they don't appear as
    /// a subsequence in that order.
    ///
    /// Returns `Some(score)` if matched (higher is better), or `None` if no match.
    fn enhanced_fuzzy_match(&self, candidate: &str, query: &str) -> Option<i64> {
        // Pass 1: Try the standard skim fuzzy match (already case-insensitive)
        if let Some(score) = self.fz_matcher.fuzzy_match(candidate, query) {
            return Some(score);
        }

        // Pass 2: Check if all query characters exist in the candidate
        // (in any order, case-insensitive, consuming each matched char once)
        let candidate_lower = candidate.to_lowercase();
        let query_lower = query.to_lowercase();

        let mut available: Vec<char> = candidate_lower.chars().collect();
        let mut all_found = true;

        for qc in query_lower.chars() {
            if let Some(pos) = available.iter().position(|&c| c == qc) {
                // Remove the matched character so it can't be reused
                available.remove(pos);
            } else {
                // A query character was not found — no match
                all_found = false;
                break;
            }
        }

        if all_found && !query.is_empty() {
            // Return a low positive score so standard skim matches always
            // rank higher than bag-of-characters matches
            Some(1)
        } else {
            None
        }
    }

    /// Check if a candidate string matches the current search input.
    ///
    /// Returns true if the input is empty (everything matches when there is
    /// no search query) or if the enhanced fuzzy match finds any match.
    /// Used throughout the code to filter which items are displayed in the list.
    fn is_match(&self, candidate: &str) -> bool {
        self.input.is_empty() || self.enhanced_fuzzy_match(candidate, &self.input).is_some()
    }

    // -------------------------------------------------------------------------
    // Pane list rebuilding
    // -------------------------------------------------------------------------

    /// Rebuild the flattened list of all panes across all tabs.
    ///
    /// Zellij provides panes grouped by tab via `PaneManifest`, but we want a
    /// single searchable list. This method collects every non-plugin pane from
    /// every tab, attaches the tab's name to each entry, and sorts by tab
    /// position for a consistent display order.
    ///
    /// Called whenever the pane manifest or tab info changes (i.e., when Zellij
    /// sends a `PaneUpdate` or `TabUpdate` event).
    fn rebuild_all_panes(&mut self) {
        let mut flat: Vec<FlatPane> = Vec::new();

        // Iterate over every tab in the pane manifest
        for (tab_pos, panes) in &self.pane_manifest.panes {
            // Look up the tab's human-readable name from tab_infos.
            // Fall back to "Tab N" if the tab info hasn't arrived yet.
            let tab_name = self
                .tab_infos
                .iter()
                .find(|t| t.position == *tab_pos)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| format!("Tab {}", tab_pos));

            // Add each non-plugin pane to the flat list
            for pane in panes {
                if !pane.is_plugin {
                    flat.push(FlatPane {
                        pane_id: pane.id,
                        title: pane.title.clone(),
                        tab_position: *tab_pos,
                        tab_name: tab_name.clone(),
                    });
                }
            }
        }

        // Sort by tab position so panes are grouped by tab in the UI
        flat.sort_by_key(|p| p.tab_position);
        self.all_panes = flat;
    }

    // -------------------------------------------------------------------------
    // Key event handling
    // -------------------------------------------------------------------------

    /// Handle a key press from the user. This is the main input dispatcher.
    ///
    /// Depending on the key pressed and the current view, this method:
    ///   - Enter: Closes the plugin and navigates to the selected item
    ///   - Backspace: Removes a character from the search input and re-searches
    ///   - Down/Up: Moves the selection highlight through matching items
    ///   - PageUp: Jumps to the top of the tab list (tab view only)
    ///   - Left/Right: Moves the cursor within the search input
    ///   - Esc/Ctrl+C: Closes the plugin without navigating
    ///   - Tab: Cycles between Tab → Pane → Session views
    ///   - Any character: Adds it to the search input and re-searches
    ///
    /// Returns `true` if the UI should be redrawn after handling the key.
    fn handle_key_event(&mut self, key: KeyWithModifier) -> bool {
        let mut should_render = true;
        match key.bare_key {
            // --- Enter: navigate to the selected item ---
            BareKey::Enter => match self.current_view {
                View::Tab => {
                    if let Some(p) = self.tab_match {
                        // Close the plugin pane by its ID (not by focus) to avoid
                        // accidentally closing a different pane, then switch tabs
                        self.close();
                        // Zellij's switch_tab_to() uses 1-indexed tab positions
                        switch_tab_to(p as u32 + 1);
                    }
                }
                View::Pane => {
                    if let Some(pane_id) = self.pane_match {
                        // IMPORTANT: Close the plugin pane by its specific ID first,
                        // before switching tabs. Using close_focus() here would be
                        // dangerous because switch_tab_to() changes which pane has
                        // focus, and close_focus() would then delete the wrong pane.
                        self.close();
                        // Switch to the tab that contains the target pane
                        if let Some(tab_pos) = self.pane_tab_position {
                            switch_tab_to(tab_pos as u32 + 1);
                        }
                        // Focus the target pane on the destination tab
                        focus_terminal_pane(pane_id, true);
                    }
                }
                View::Session => {
                    if let Some(sess) = &self.session_match {
                        // Close the plugin pane by its ID before switching sessions
                        self.close();
                        switch_session(Some(sess));
                    }
                }
            },

            // --- Backspace: remove a character and re-run fuzzy search ---
            BareKey::Backspace => {
                if self.remove_input_at_index() {
                    match self.current_view {
                        View::Tab => {
                            self.fuzzy_find_tab();
                        }
                        View::Pane => {
                            self.fuzzy_find_pane();
                        }
                        View::Session => {
                            self.fuzzy_find_session();
                        }
                    }
                }
                should_render = true;
            }

            // --- Down arrow: move selection to the next matching item ---
            BareKey::Down => {
                match self.current_view {
                    View::Tab => {
                        self.move_down_tab();
                    }
                    View::Pane => {
                        self.move_down_pane();
                    }
                    View::Session => {
                        self.move_down_session();
                    }
                }
                should_render = true;
            }

            // --- PageUp: jump to the top of the tab list ---
            BareKey::PageUp => {
                if let View::Tab = self.current_view {
                    self.seek_tab(0);
                }
                should_render = true;
            }

            // --- Up arrow: move selection to the previous matching item ---
            BareKey::Up => {
                match self.current_view {
                    View::Tab => {
                        self.move_up_tab();
                    }
                    View::Pane => {
                        self.move_up_pane();
                    }
                    View::Session => {
                        self.move_up_session();
                    }
                }
                should_render = true;
            }

            // --- Left/Right: move cursor within the search input ---
            BareKey::Left => {
                if self.input_cusror_index > 0 {
                    self.input_cusror_index -= 1;
                }
                should_render = true;
            }
            BareKey::Right => {
                if self.input_cusror_index < self.input.len() {
                    self.input_cusror_index += 1;
                }
                should_render = true;
            }

            // --- Esc / Ctrl+C: close the plugin without navigating ---
            BareKey::Esc => {
                self.close();
                should_render = true;
            }
            BareKey::Char('c') if key.has_modifiers(&[KeyModifier::Ctrl]) => {
                self.close();
                should_render = true;
            }

            // --- Tab key: cycle to the next view (Tab → Pane → Session) ---
            BareKey::Tab => {
                self.change_mode();
                should_render = true;
            }

            // --- Any other character: add to search input and re-search ---
            BareKey::Char(c) => {
                if self.insert_input_at_index(c) {
                    match self.current_view {
                        View::Tab => {
                            self.fuzzy_find_tab();
                        }
                        View::Pane => {
                            self.fuzzy_find_pane();
                        }
                        View::Session => {
                            self.fuzzy_find_session();
                        }
                    }
                }
                should_render = true;
            }

            // Ignore any other keys
            _ => (),
        };

        should_render
    }

    // -------------------------------------------------------------------------
    // Plugin lifecycle helpers
    // -------------------------------------------------------------------------

    /// Find and record which tab is currently focused (active).
    ///
    /// Iterates through all tabs and looks for the one marked as `active`.
    /// If no tab match has been set yet (e.g., on first load), it defaults
    /// the selection to the active tab so the user sees it highlighted.
    fn get_focused_tab(&mut self) {
        for (i, t) in self.tab_infos.iter().enumerate() {
            if t.active {
                self.focus_tab_pos = t.position;
                if self.tab_match.is_none() {
                    self.tab_match = Some(i);
                    // Only update the highlight position if we're in Tab view
                    if let View::Tab = self.current_view {
                        self.result_index = i;
                    }
                }
            }
        }
    }

    /// Close the plugin pane by its specific ID.
    ///
    /// Uses `close_plugin_pane()` with the plugin's own ID rather than
    /// `close_focus()`, which would close whatever pane happens to be focused.
    /// This prevents accidentally deleting other panes when focus has shifted
    /// (e.g., after a `switch_tab_to()` call).
    fn close(&self) {
        close_plugin_pane(get_plugin_ids().plugin_id);
    }

    /// Cycle to the next view: Tab → Pane → Session → Tab.
    ///
    /// Resets the search input when switching views. When switching to the
    /// Pane view, initializes the pane selection to the first available pane.
    /// When switching to Tab or Session view, restores the previously matched
    /// tab position so the user doesn't lose their place.
    fn change_mode(&mut self) {
        // Reset the search input when changing views
        self.input = String::default();
        self.input_cusror_index = 0;

        // Advance to the next view in the cycle
        match self.current_view {
            View::Tab => {
                self.current_view = View::Pane;
            }
            View::Pane => {
                self.current_view = View::Session;
            }
            View::Session => {
                self.current_view = View::Tab;
            }
        }

        if let View::Pane = self.current_view {
            // When switching to pane view, reset pane selection state
            // and select the first pane in the flattened cross-tab list
            self.pane_match = None;
            self.pane_title_match = String::default();
            self.pane_tab_position = None;
            self.result_index = 0;

            self.select_pane_at_index();
            return;
        }

        // When switching to Tab or Session view, restore the highlight to
        // the previously matched tab so the user doesn't lose their place
        if let Some(i) = self.tab_match {
            self.result_index = i;
        }
    }

    // -------------------------------------------------------------------------
    // Tab search and navigation
    // -------------------------------------------------------------------------

    /// Fuzzy-find the best matching tab based on the current search input.
    ///
    /// Iterates through all tabs and picks the one with the highest fuzzy match
    /// score. If no tab matches the search, defaults to the currently focused
    /// tab (so the user always has something selected).
    fn fuzzy_find_tab(&mut self) {
        let mut best_score = 0;

        // Reset the current match before searching
        self.tab_match = None;
        self.result_index = 0;
        for (i, t) in self.tab_infos.iter().enumerate() {
            if let Some(score) = self.enhanced_fuzzy_match(t.name.as_str(), &self.input) {
                if score > best_score {
                    best_score = score;
                    self.tab_match = Some(i);
                    self.result_index = i;
                }
            }
        }

        // If no tab matched the search, default to the currently focused tab
        if self.tab_match.is_none() {
            self.tab_match = Some(self.focus_tab_pos);
            self.result_index = self.focus_tab_pos;
        }
    }

    /// Jump to a specific index in the tab list (used by PageUp to go to top).
    ///
    /// Sets the result_index to the given position and finds the first tab
    /// at that index which matches the current search input.
    fn seek_tab(&mut self, idx: usize) {
        self.result_index = idx;
        for (i, t) in self.tab_infos.iter().enumerate() {
            if self.is_match(t.name.as_str()) && i == self.result_index {
                self.tab_match = Some(i);
                self.result_index = i;
                break;
            }
        }
    }

    /// Move the tab selection down to the next matching tab.
    ///
    /// Skips tabs that don't match the current search input. If the end of
    /// the list is reached, wraps around to the first matching tab.
    fn move_down_tab(&mut self) {
        let mut first_match = None;
        let mut seek_result = false;
        let mut found_next = None;

        for (i, t) in self.tab_infos.iter().enumerate() {
            if self.is_match(t.name.as_str()) {
                // Remember the very first match (for wrap-around)
                if first_match.is_none() {
                    first_match = Some(i);
                }
                // When we find the currently selected item, start seeking the next
                if i == self.result_index {
                    seek_result = true;
                    continue;
                }
                // The first match after the current selection becomes the new selection
                if seek_result {
                    found_next = Some(i);
                    self.tab_match = Some(i);
                    self.result_index = i;
                    break;
                }
            }
        }

        // If no next match was found, wrap around to the first match
        if found_next.is_none() {
            if let Some(i) = first_match {
                self.tab_match = Some(i);
                self.result_index = i;
            }
        }
    }

    /// Move the tab selection up to the previous matching tab.
    ///
    /// Skips tabs that don't match the current search input. If the beginning
    /// of the list is reached, wraps around to the last matching tab.
    fn move_up_tab(&mut self) {
        let mut prev_match = None;
        let mut last_match = None;

        for (i, t) in self.tab_infos.iter().enumerate() {
            if self.is_match(t.name.as_str()) {
                // If we've reached the current selection and have a previous match, stop
                if i == self.result_index && prev_match.is_some() {
                    break;
                }
                prev_match = Some(i);
                last_match = Some(i);
            }
        }

        // Use the previous match if one was found
        if let Some(i) = prev_match {
            self.tab_match = Some(i);
            self.result_index = i;
            return;
        }

        // Otherwise wrap around to the last match
        if let Some(i) = last_match {
            self.tab_match = Some(i);
            self.result_index = i;
        }
    }

    // -------------------------------------------------------------------------
    // Session search and navigation
    // -------------------------------------------------------------------------

    /// Move the session selection down to the next matching session.
    ///
    /// Works the same way as move_down_tab() but operates on the sessions list.
    /// Wraps around to the first match if the end of the list is reached.
    fn move_down_session(&mut self) {
        let mut first_match = None;
        let mut seek_result = false;
        let mut found_next = None;

        for (i, session) in self.sessions.iter().enumerate() {
            if self.is_match(session) {
                // Remember the very first match (for wrap-around)
                if first_match.is_none() {
                    first_match = Some(i);
                }
                // When we find the currently selected item, start seeking the next
                if i == self.result_index {
                    seek_result = true;
                    continue;
                }
                // The first match after the current selection becomes the new selection
                if seek_result {
                    if let Some(sess) = self.sessions.get(i) {
                        found_next = Some(i);
                        self.session_match = Some(sess.to_owned());
                        self.result_index = i;
                        break;
                    }
                }
            }
        }

        // If no next match was found, wrap around to the first match
        if found_next.is_none() {
            if let Some(i) = first_match {
                if let Some(sess) = self.sessions.get(i) {
                    self.session_match = Some(sess.to_owned());
                    self.result_index = i;
                }
            }
        }
    }

    /// Move the session selection up to the previous matching session.
    ///
    /// Works the same way as move_up_tab() but operates on the sessions list.
    /// Wraps around to the last match if the beginning of the list is reached.
    fn move_up_session(&mut self) {
        let mut prev_match = None;
        let mut last_match = None;

        for (i, session) in self.sessions.iter().enumerate() {
            if self.is_match(session) {
                // If we've reached the current selection and have a previous match, stop
                if i == self.result_index && prev_match.is_some() {
                    break;
                }
                prev_match = Some(i);
                last_match = Some(i);
            }
        }

        // Use the previous match if one was found
        if let Some(i) = prev_match {
            if let Some(sess) = self.sessions.get(i) {
                self.session_match = Some(sess.to_owned());
                self.result_index = i;
            }
            return;
        }

        // Otherwise wrap around to the last match
        if let Some(i) = last_match {
            if let Some(sess) = self.sessions.get(i) {
                self.session_match = Some(sess.to_owned());
                self.result_index = i;
            }
        }
    }

    /// Fuzzy-find the best matching session based on the current search input.
    ///
    /// Iterates through all sessions and picks the one with the highest fuzzy
    /// match score. Resets the current session match before searching.
    fn fuzzy_find_session(&mut self) {
        let mut best_score = 0;

        // Reset the current session match before searching
        self.session_match = None;
        for (i, session) in self.sessions.iter().enumerate() {
            if let Some(score) = self.enhanced_fuzzy_match(session, &self.input) {
                if score > best_score {
                    best_score = score;
                    self.result_index = i;
                    self.session_match = Some(session.to_owned());
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // Pane search and navigation (cross-tab)
    // -------------------------------------------------------------------------

    /// Fuzzy-find the best matching pane across ALL tabs in the session.
    ///
    /// Iterates through the flattened `all_panes` list (which contains panes
    /// from every tab) and picks the pane with the highest fuzzy match score.
    /// Also records which tab the matched pane belongs to, so we can switch
    /// to that tab when the user presses Enter.
    fn fuzzy_find_pane(&mut self) {
        let mut best_score = 0;

        // Reset the current pane selection
        self.pane_match = None;
        self.pane_title_match = String::default();
        self.pane_tab_position = None;
        self.result_index = 0;

        // Search through all panes across all tabs
        for (i, flat) in self.all_panes.iter().enumerate() {
            if let Some(score) = self.enhanced_fuzzy_match(flat.title.as_str(), &self.input) {
                if score > best_score {
                    best_score = score;
                    self.pane_match = Some(flat.pane_id);
                    self.pane_title_match = flat.title.clone();
                    self.pane_tab_position = Some(flat.tab_position);
                    self.result_index = i;
                }
            }
        }
    }

    /// Select the pane at the current `result_index` in the flattened pane list.
    ///
    /// Called when switching to the Pane view to initialize the selection.
    /// Finds the first pane that matches the current search input and is at
    /// the current `result_index` position.
    fn select_pane_at_index(&mut self) {
        for (i, flat) in self.all_panes.iter().enumerate() {
            if self.is_match(flat.title.as_str()) && i == self.result_index {
                self.pane_match = Some(flat.pane_id);
                self.pane_title_match = flat.title.clone();
                self.pane_tab_position = Some(flat.tab_position);
                break;
            }
        }
    }

    /// Move the pane selection down to the next matching pane.
    ///
    /// Works the same way as move_down_tab() but operates on the flattened
    /// cross-tab pane list. Wraps around to the first match if the end of
    /// the list is reached.
    fn move_down_pane(&mut self) {
        let mut first_match = None;
        let mut seek_result = false;
        let mut found_next = None;

        // Reset current selection before searching
        self.pane_match = None;
        self.pane_title_match = String::default();
        self.pane_tab_position = None;

        for (i, flat) in self.all_panes.iter().enumerate() {
            if self.is_match(flat.title.as_str()) {
                // Remember the very first match (for wrap-around)
                if first_match.is_none() {
                    first_match = Some(i);
                }
                // When we find the currently selected item, start seeking the next
                if i == self.result_index {
                    seek_result = true;
                    continue;
                }
                // The first match after the current selection becomes the new selection
                if seek_result {
                    self.pane_match = Some(flat.pane_id);
                    self.pane_title_match = flat.title.clone();
                    self.pane_tab_position = Some(flat.tab_position);
                    found_next = Some(i);
                    self.result_index = i;
                    break;
                }
            }
        }

        // If no next match was found, wrap around to the first match
        if found_next.is_none() {
            if let Some(i) = first_match {
                if let Some(flat) = self.all_panes.get(i) {
                    self.pane_match = Some(flat.pane_id);
                    self.pane_title_match = flat.title.clone();
                    self.pane_tab_position = Some(flat.tab_position);
                    self.result_index = i;
                }
            }
        }
    }

    /// Move the pane selection up to the previous matching pane.
    ///
    /// Works the same way as move_up_tab() but operates on the flattened
    /// cross-tab pane list. Wraps around to the last match if the beginning
    /// of the list is reached.
    fn move_up_pane(&mut self) {
        let mut prev_match = None;
        let mut last_match = None;

        // Reset current selection before searching
        self.pane_match = None;
        self.pane_title_match = String::default();
        self.pane_tab_position = None;

        for (i, flat) in self.all_panes.iter().enumerate() {
            if self.is_match(flat.title.as_str()) {
                // If we've reached the current selection and have a previous match, stop
                if i == self.result_index && prev_match.is_some() {
                    break;
                }
                prev_match = Some(i);
                last_match = Some(i);
            }
        }

        // Use the previous match if one was found
        if let Some(i) = prev_match {
            if let Some(flat) = self.all_panes.get(i) {
                self.pane_match = Some(flat.pane_id);
                self.pane_title_match = flat.title.clone();
                self.pane_tab_position = Some(flat.tab_position);
                self.result_index = i;
            }
            return;
        }

        // Otherwise wrap around to the last match
        if let Some(i) = last_match {
            if let Some(flat) = self.all_panes.get(i) {
                self.pane_match = Some(flat.pane_id);
                self.pane_title_match = flat.title.clone();
                self.pane_tab_position = Some(flat.tab_position);
                self.result_index = i;
            }
        }
    }

    // -------------------------------------------------------------------------
    // Search input editing
    // -------------------------------------------------------------------------

    /// Remove the character at the cursor position from the search input.
    ///
    /// Handles three cases:
    ///   - Empty input: nothing to remove
    ///   - Cursor in the middle or at the end: remove the character before the cursor
    ///   - Cursor at position 0: remove the first character
    ///
    /// Returns `true` if the input actually changed (so the caller knows whether
    /// to re-run the fuzzy search).
    fn remove_input_at_index(&mut self) -> bool {
        if self.input.is_empty() {
            self.input.pop();
        } else if self.input_cusror_index > 0 && self.input_cusror_index <= self.input.len() {
            self.input.remove(self.input_cusror_index - 1);
            // Move the cursor back to stay in the right position
            self.input_cusror_index -= 1;
            return true;
        } else if self.input_cusror_index == 0 {
            self.input.remove(0);
        }
        false
    }

    /// Insert a character at the cursor position in the search input.
    ///
    /// Handles three cases:
    ///   - Empty input: append the character
    ///   - Cursor in the middle or at the end: insert at cursor position
    ///   - Cursor at position 0: insert at the beginning
    ///
    /// Returns `true` if the input actually changed (so the caller knows whether
    /// to re-run the fuzzy search).
    fn insert_input_at_index(&mut self, c: char) -> bool {
        if self.input.is_empty() {
            self.input.push(c);
            // Advance the cursor past the new character
            self.input_cusror_index += 1;
            return true;
        } else if self.input_cusror_index > 0 && self.input_cusror_index <= self.input.len() {
            self.input.insert(self.input_cusror_index, c);
            // Advance the cursor past the new character
            self.input_cusror_index += 1;
            return true;
        } else if self.input_cusror_index == 0 {
            self.input.insert(0, c);
            self.input_cusror_index += 1;
        }
        false
    }

    // -------------------------------------------------------------------------
    // UI rendering helpers
    // -------------------------------------------------------------------------

    /// Print the search input prompt line.
    ///
    /// Shows a cyan "> " prompt followed by the search input with a cursor
    /// indicator (┃). If the input is empty, shows a dimmed placeholder text
    /// "search pattern" to hint at what the user should type.
    fn print_prompt(&self, _rows: usize, _cols: usize) {
        let prompt = " > ".cyan().bold().to_string();
        if self.input.is_empty() {
            // Show placeholder text when no search input has been typed
            println!(
                "{} {}{}",
                prompt,
                "┃".bold().white(),
                "search pattern".dimmed().italic(),
            );
        } else {
            self.print_non_empty_input_prompt(prompt);
        }
    }

    /// Print the search input prompt when there is text in the input.
    ///
    /// Splits the input at the cursor position and renders the cursor
    /// indicator (┃) between the two halves. This gives the user visual
    /// feedback about where new characters will be inserted.
    fn print_non_empty_input_prompt(&self, prompt: String) {
        match self.input_cusror_index.cmp(&self.input.len()) {
            // Cursor is at the end of the input (most common case)
            std::cmp::Ordering::Equal => {
                println!("{} {}{}", prompt, self.input.dimmed(), "┃".bold().white(),);
            }
            // Cursor is in the middle of the input
            std::cmp::Ordering::Less => {
                let copy = self.input.clone();
                let (before_curs, after_curs) = copy.split_at(self.input_cusror_index);

                println!(
                    "{} {}{}{}",
                    prompt,
                    before_curs.dimmed(),
                    "┃".bold().white(),
                    after_curs.dimmed()
                );
            }
            // Cursor is past the end (should not happen, but handled for safety)
            std::cmp::Ordering::Greater => (),
        }
    }
}

// =============================================================================
// Zellij plugin registration and trait implementation.
//
// The `register_plugin!` macro tells Zellij which struct implements the plugin.
// Zellij will create a `State` instance using `Default::default()` and then
// call `load()`, `update()`, and `render()` on it throughout the plugin's life.
// =============================================================================

register_plugin!(State);
impl ZellijPlugin for State {
    /// Called once when the plugin is first loaded by Zellij.
    ///
    /// Stores the user's configuration, requests the permissions needed to
    /// read application state (tabs, panes, sessions) and change it (switch
    /// tabs, close panes), subscribes to the events we care about, and
    /// renames the plugin pane to "Zigzag".
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.userspace_configuration = configuration;

        // Request permissions:
        //   - ReadApplicationState: needed to receive tab, pane, and session updates
        //   - ChangeApplicationState: needed to switch tabs, focus panes, close plugin
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);

        // Subscribe to the events we need:
        //   - ModeUpdate: Zellij input mode changes (not currently used, but subscribed)
        //   - TabUpdate: fired when tabs are created, renamed, closed, or reordered
        //   - PaneUpdate: fired when panes are created, closed, or changed
        //   - Key: every key press while the plugin is focused
        //   - SessionUpdate: fired when sessions change
        subscribe(&[
            EventType::ModeUpdate,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::Key,
            EventType::SessionUpdate,
        ]);

        // Give the plugin pane a recognizable name in the Zellij UI
        rename_plugin_pane(get_plugin_ids().plugin_id, "Zigzag");
    }

    /// Called each time an event arrives from Zellij.
    ///
    /// Updates the plugin's internal state based on the event type:
    ///   - TabUpdate: stores the new tab list and identifies the focused tab
    ///   - PaneUpdate: stores the new pane manifest
    ///   - SessionUpdate: stores the new session names
    ///   - Key: delegates to handle_key_event() for input processing
    ///
    /// Both TabUpdate and PaneUpdate trigger a rebuild of the flattened
    /// cross-tab pane list so the Pane view always has up-to-date data.
    ///
    /// Returns `true` if the UI should be redrawn.
    fn update(&mut self, event: Event) -> bool {
        let mut should_render = true;
        match event {
            Event::TabUpdate(tab_info) => {
                self.tab_infos = tab_info;
                self.get_focused_tab();
                // Rebuild the flattened pane list since tab names may have changed
                self.rebuild_all_panes();
                should_render = true;
            }
            Event::PaneUpdate(pane_manifest) => {
                self.pane_manifest = pane_manifest;
                // Rebuild the flattened pane list with the new pane data
                self.rebuild_all_panes();
                should_render = true;
            }
            Event::SessionUpdate(session_infos, _) => {
                // Extract just the session names from the session info structs
                self.sessions = session_infos
                    .into_iter()
                    .map(|session_info| session_info.name)
                    .collect();
            }
            Event::Key(key) => {
                should_render = self.handle_key_event(key);
            }
            _ => (),
        };

        should_render
    }

    /// Called by Zellij to draw the plugin UI.
    ///
    /// Receives the available `rows` and `cols` so the output can fit within
    /// the plugin pane. The rendering has three main sections:
    ///   1. The view selector ribbon (Tab / Pane / Session, with the active
    ///      one highlighted)
    ///   2. The search prompt showing the user's input with a cursor
    ///   3. The list of matching items for the current view, with the selected
    ///      item highlighted in blue and the rest dimmed
    ///
    /// At the bottom, shows which item is currently selected. In debug mode
    /// (set "debug" to "true" in plugin config), also shows internal state.
    fn render(&mut self, rows: usize, cols: usize) {
        // Check if debug mode is enabled in the plugin configuration
        let debug = self.userspace_configuration.get("debug");

        // Track how many lines we've printed so we don't overflow the pane.
        // Start at 4 to reserve space for the header and footer lines.
        let mut count = 4;

        // --- Section 1: View selector ribbon ---
        // Show three ribbons at the top; the active view is highlighted
        match self.current_view {
            View::Tab => {
                print_ribbon_with_coordinates(
                    Text::new("Tabs Selector").selected(),
                    1,
                    0,
                    None,
                    None,
                );
                print_ribbon_with_coordinates(Text::new("Panes Selector"), 18, 0, None, None);
                print_ribbon_with_coordinates(Text::new("Sessions Selector"), 36, 0, None, None);
                println!();
                println!();
            }
            View::Pane => {
                print_ribbon_with_coordinates(Text::new("Tabs Selector"), 1, 0, None, None);
                print_ribbon_with_coordinates(
                    Text::new("Panes Selector").selected(),
                    18,
                    0,
                    None,
                    None,
                );
                print_ribbon_with_coordinates(Text::new("Sessions Selector"), 36, 0, None, None);
                println!();
                println!();
            }
            View::Session => {
                print_ribbon_with_coordinates(Text::new("Tabs Selector"), 1, 0, None, None);
                print_ribbon_with_coordinates(Text::new("Panes Selector"), 18, 0, None, None);
                print_ribbon_with_coordinates(
                    Text::new("Sessions Selector").selected(),
                    36,
                    0,
                    None,
                    None,
                );
                println!();
                println!();
            }
        }

        count += 1;

        // --- Section 2: Search prompt ---
        self.print_prompt(rows, cols);
        count += 1;

        // --- Section 3: List of matching items for the current view ---
        match self.current_view {
            View::Tab => {
                println!("Tabs: ");
                count += 1;

                // Display each tab that matches the current search input
                for (i, t) in self.tab_infos.iter().enumerate() {
                    if self.is_match(t.name.as_str()) {
                        // Stop printing if we would overflow the available rows
                        if count >= rows - 4 {
                            println!(" - {}", "...".dimmed());
                            break;
                        }
                        // Highlight the currently selected tab in blue
                        if i == self.result_index {
                            println!(" - {}", t.name.blue().bold());
                        } else {
                            println!(" - {}", t.name.dimmed());
                        }
                        count += 1;
                    }
                }

                // Show which tab is currently selected at the bottom
                println!();
                if let Some(m) = self.tab_match {
                    if let Some(t) = self.tab_infos.get(m) {
                        println!(
                            "{} {}",
                            color_bold(WHITE, "Selected Tab ->"),
                            t.name.as_str().blue().bold()
                        );
                    }
                } else {
                    println!(
                        "{} {}",
                        color_bold(WHITE, "Selected Tab ->"),
                        "No matches found".dimmed()
                    );
                }
            }

            View::Pane => {
                // Display ALL panes across ALL tabs (not just the current tab)
                println!("Panes (all tabs): ");

                // Iterate through the flattened list of all panes
                for (i, flat) in self.all_panes.iter().enumerate() {
                    if self.is_match(flat.title.as_str()) {
                        // Stop printing if we would overflow the available rows
                        if count >= rows - 4 {
                            println!(" - {}", "...".dimmed());
                            break;
                        }
                        // Show the pane title with its tab name in brackets so the
                        // user knows which tab each pane belongs to
                        if i == self.result_index {
                            println!(
                                " - {} {}",
                                flat.title.blue().bold(),
                                format!("[{}]", flat.tab_name).dimmed()
                            );
                        } else {
                            println!(
                                " - {} {}",
                                flat.title.dimmed(),
                                format!("[{}]", flat.tab_name).dimmed()
                            );
                        }
                        count += 1;
                    }
                }

                // Show which pane is currently selected at the bottom
                println!();
                if !self.pane_title_match.is_empty() {
                    println!(
                        "{} {}",
                        color_bold(WHITE, "Selected Pane ->"),
                        self.pane_title_match.as_str().blue().bold()
                    );
                } else {
                    println!(
                        "{} {}",
                        color_bold(WHITE, "Selected Pane ->"),
                        "No matches found".dimmed()
                    );
                }

                // Show which tab the selected pane belongs to
                if let Some(tab_pos) = self.pane_tab_position {
                    // Look up the tab name from the tab position
                    let tab_name = self
                        .tab_infos
                        .iter()
                        .find(|t| t.position == tab_pos)
                        .map(|t| t.name.as_str())
                        .unwrap_or("Unknown");
                    println!(
                        "{} {}",
                        color_bold(WHITE, "In Tab ->"),
                        tab_name.blue().bold()
                    );
                }
            }

            View::Session => {
                println!("Sessions: ");

                // Display each session that matches the current search input
                for (i, session) in self.sessions.iter().enumerate() {
                    if self.is_match(session) {
                        // Stop printing if we would overflow the available rows
                        if count >= rows - 4 {
                            println!(" - {}", "...".dimmed());
                            break;
                        }
                        // Highlight the currently selected session in blue
                        if i == self.result_index {
                            println!(" - {}", session.blue().bold());
                        } else {
                            println!(" - {}", session.dimmed());
                        }
                        count += 1;
                    }
                }

                // Show which session is currently selected at the bottom
                println!();
                if let Some(sess) = &self.session_match {
                    println!(
                        "{} {}",
                        color_bold(WHITE, "Selected Session ->"),
                        sess.as_str().blue().bold()
                    );
                } else {
                    println!(
                        "{} {}",
                        color_bold(WHITE, "Selected Session ->"),
                        "No matches found".dimmed()
                    );
                }
            }
        }

        // --- Optional debug output ---
        // When "debug" is set to "true" in the plugin configuration, print
        // internal state values for troubleshooting
        if debug.is_some_and(|x| x == "true") {
            println!("input: {}", self.input);
            println!("Cursor: {}", self.input_cusror_index);
            println!("len: {}", self.input.len());
            println!("tab match: {}", self.tab_match.unwrap_or(42));
            println!("pane match: {}", self.pane_match.unwrap_or(42));
            println!("focussed tab : {}", self.focus_tab_pos);
            println!("result_index: {}", self.result_index);
            println!(
                "{} {:#?}",
                color_bold(GREEN, "Runtime configuration:"),
                self.userspace_configuration
            );
        }
    }
}

// =============================================================================
// Color constants and helpers
// =============================================================================

/// ANSI 256-color codes used for styling text output in the plugin UI.
pub const CYAN: u8 = 51;
pub const GRAY_LIGHT: u8 = 238;
pub const GRAY_DARK: u8 = 245;
pub const WHITE: u8 = 15;
pub const BLACK: u8 = 16;
pub const RED: u8 = 124;
pub const GREEN: u8 = 154;
pub const ORANGE: u8 = 166;

/// Format text with a bold style and a specific ANSI 256-color code.
/// Used for colored labels like "Selected Tab ->" in the UI output.
fn color_bold(color: u8, text: &str) -> String {
    format!("{}", Style::new().fg(Fixed(color)).bold().paint(text))
}
