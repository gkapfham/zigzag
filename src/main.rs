use ansi_term::{Colour::Fixed, Style};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use owo_colors::OwoColorize;

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

/// The three main views the user can switch between using the Tab key.
enum View {
    Session,
    Tab,
    Pane,
}

/// A flattened representation of a single pane from any tab in the session.
/// This is used to build a searchable list of ALL panes across ALL tabs,
/// so the user can fuzzy-find and jump to any pane regardless of which tab it lives in.
#[derive(Clone)]
struct FlatPane {
    /// The pane's unique numeric ID (used by Zellij to focus it)
    pane_id: u32,
    /// The display title of the pane (e.g., the running command or custom name)
    title: String,
    /// The 0-indexed position of the tab this pane belongs to
    tab_position: usize,
    /// The human-readable name of the tab this pane belongs to
    tab_name: String,
}

struct State {
    /// User-provided plugin configuration from the Zellij config file
    userspace_configuration: BTreeMap<String, String>,

    /// Which view (Tab, Pane, or Session) is currently active
    current_view: View,
    /// The 0-indexed position of the currently focused tab
    focus_tab_pos: usize,
    /// The index of the currently highlighted item in the active list
    result_index: usize,
    /// All tab information received from Zellij
    tab_infos: Vec<TabInfo>,
    /// Raw pane manifest from Zellij (panes grouped by tab position)
    pane_manifest: PaneManifest,
    /// The current search/filter input string typed by the user
    input: String,
    /// The cursor position within the input string (for Left/Right navigation)
    input_cusror_index: usize,
    /// The index of the best-matching tab (into tab_infos)
    tab_match: Option<usize>,
    /// The name of the best-matching session
    session_match: Option<String>,
    /// The pane ID of the best-matching pane
    pane_match: Option<u32>,
    /// The display title of the best-matching pane
    pane_title_match: String,
    /// The tab position that the best-matching pane belongs to
    pane_tab_position: Option<usize>,
    /// A flattened, sorted list of ALL non-plugin panes across ALL tabs.
    /// This is rebuilt whenever the pane manifest or tab info changes.
    all_panes: Vec<FlatPane>,
    /// All session names in the current Zellij instance
    sessions: Vec<String>,
    /// The fuzzy matcher instance (configured for case-insensitive matching)
    fz_matcher: SkimMatcherV2,
}

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

impl State {
    /// Enhanced fuzzy match: first tries standard skim fuzzy matching (case-insensitive),
    /// then falls back to a "bag of characters" match where all query characters must
    /// appear in the candidate (in any order, case-insensitive). This handles mistypes
    /// like "ageM" matching "Manager" — all chars a, g, e, m are present in "manager".
    fn enhanced_fuzzy_match(&self, candidate: &str, query: &str) -> Option<i64> {
        // Primary: standard skim fuzzy match (already case-insensitive)
        if let Some(score) = self.fz_matcher.fuzzy_match(candidate, query) {
            return Some(score);
        }

        // Fallback: check if all query characters exist in the candidate
        // (in any order, case-insensitive, consuming each matched char once)
        let candidate_lower = candidate.to_lowercase();
        let query_lower = query.to_lowercase();

        let mut available: Vec<char> = candidate_lower.chars().collect();
        let mut all_found = true;

        for qc in query_lower.chars() {
            if let Some(pos) = available.iter().position(|&c| c == qc) {
                available.remove(pos);
            } else {
                all_found = false;
                break;
            }
        }

        if all_found && !query.is_empty() {
            // Return a low positive score so skim primary matches rank higher
            Some(1)
        } else {
            None
        }
    }

    /// Check if a candidate matches the current input (for display filtering).
    /// Returns true if the input is empty (everything matches) or if the
    /// enhanced fuzzy match finds any match.
    fn is_match(&self, candidate: &str) -> bool {
        self.input.is_empty() || self.enhanced_fuzzy_match(candidate, &self.input).is_some()
    }

    /// Rebuild the flattened list of all panes across all tabs.
    /// This is called whenever the pane manifest or tab info changes.
    /// It collects every non-plugin pane from every tab, pairs it with
    /// the tab's name, and sorts by tab position for consistent display order.
    fn rebuild_all_panes(&mut self) {
        let mut flat: Vec<FlatPane> = Vec::new();

        // Iterate over every tab in the pane manifest
        for (tab_pos, panes) in &self.pane_manifest.panes {
            // Look up the tab's human-readable name from tab_infos
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

    fn handle_key_event(&mut self, key: KeyWithModifier) -> bool {
        let mut should_render = true;
        match key.bare_key {
            BareKey::Enter => match self.current_view {
                View::Tab => {
                    if let Some(p) = self.tab_match {
                        // Close the plugin pane by its ID (not by focus) to avoid
                        // accidentally closing a different pane, then switch tabs
                        self.close();
                        switch_tab_to(p as u32 + 1);
                    }
                }
                View::Pane => {
                    if let Some(pane_id) = self.pane_match {
                        // IMPORTANT: close the plugin pane by its specific ID first,
                        // before switching tabs. Using close_focus() here would be
                        // dangerous because switch_tab_to() changes which pane has
                        // focus, and close_focus() would then delete the wrong pane.
                        self.close();
                        // Switch to the tab that contains the target pane
                        if let Some(tab_pos) = self.pane_tab_position {
                            // switch_tab_to uses 1-indexed tab positions
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
            BareKey::PageUp => {
                if let View::Tab = self.current_view {
                    self.seek_tab(0);
                }

                should_render = true;
            }
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

            BareKey::Esc => {
                self.close();
                should_render = true;
            }
            BareKey::Char('c') if key.has_modifiers(&[KeyModifier::Ctrl]) => {
                self.close();
                should_render = true;
            }

            BareKey::Tab => {
                self.change_mode();
                should_render = true;
            }
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
            _ => (),
        };

        should_render
    }

    /// Close current plugins and its hepler pane
    /// get the focused tab position
    fn get_focused_tab(&mut self) {
        for (i, t) in self.tab_infos.iter().enumerate() {
            if t.active {
                self.focus_tab_pos = t.position;
                if self.tab_match.is_none() {
                    self.tab_match = Some(i);

                    if let View::Tab = self.current_view {
                        self.result_index = i;
                    }
                }
            }
        }
    }

    fn close(&self) {
        close_plugin_pane(get_plugin_ids().plugin_id);
    }

    fn change_mode(&mut self) {
        // reset input
        self.input = String::default();
        self.input_cusror_index = 0;

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

        // tab view
        if let Some(i) = self.tab_match {
            self.result_index = i;
        }
    }

    fn fuzzy_find_tab(&mut self) {
        let mut best_score = 0;

        // reset match
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

        // if no found default to focus tab
        if self.tab_match.is_none() {
            self.tab_match = Some(self.focus_tab_pos);
            self.result_index = self.focus_tab_pos;
        }
    }

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

    fn move_down_tab(&mut self) {
        let mut first_match = None;
        let mut seek_result = false;
        let mut found_next = None;

        for (i, t) in self.tab_infos.iter().enumerate() {
            if self.is_match(t.name.as_str()) {
                if first_match.is_none() {
                    first_match = Some(i);
                }

                if i == self.result_index {
                    seek_result = true;
                    continue;
                }

                if seek_result {
                    found_next = Some(i);
                    self.tab_match = Some(i);
                    self.result_index = i;
                    break;
                }
            }
        }

        if found_next.is_none() {
            if let Some(i) = first_match {
                self.tab_match = Some(i);
                self.result_index = i;
            }
        }
    }

    fn move_up_tab(&mut self) {
        let mut prev_match = None;
        let mut last_match = None;

        for (i, t) in self.tab_infos.iter().enumerate() {
            if self.is_match(t.name.as_str()) {
                if i == self.result_index && prev_match.is_some() {
                    break;
                }
                prev_match = Some(i);
                last_match = Some(i);
            }
        }

        if let Some(i) = prev_match {
            self.tab_match = Some(i);
            self.result_index = i;
            return;
        }

        if let Some(i) = last_match {
            self.tab_match = Some(i);
            self.result_index = i;
        }
    }

    fn move_down_session(&mut self) {
        let mut first_match = None;
        let mut seek_result = false;
        let mut found_next = None;

        for (i, session) in self.sessions.iter().enumerate() {
            if self.is_match(session) {
                if first_match.is_none() {
                    first_match = Some(i);
                }

                if i == self.result_index {
                    seek_result = true;
                    continue;
                }

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

        if found_next.is_none() {
            if let Some(i) = first_match {
                if let Some(sess) = self.sessions.get(i) {
                    self.session_match = Some(sess.to_owned());
                    self.result_index = i;
                }
            }
        }
    }

    fn move_up_session(&mut self) {
        let mut prev_match = None;
        let mut last_match = None;

        for (i, session) in self.sessions.iter().enumerate() {
            if self.is_match(session) {
                if i == self.result_index && prev_match.is_some() {
                    break;
                }
                prev_match = Some(i);
                last_match = Some(i);
            }
        }

        if let Some(i) = prev_match {
            if let Some(sess) = self.sessions.get(i) {
                self.session_match = Some(sess.to_owned());
                self.result_index = i;
            }
            return;
        }
        if let Some(i) = last_match {
            if let Some(sess) = self.sessions.get(i) {
                self.session_match = Some(sess.to_owned());
                self.result_index = i;
            }
        }
    }

    fn fuzzy_find_session(&mut self) {
        let mut best_score = 0;

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

    /// Fuzzy-find the best matching pane across ALL tabs.
    /// Iterates through the flattened all_panes list and picks
    /// the pane with the highest fuzzy match score.
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

    /// Select the pane at the current result_index in the flattened pane list.
    /// This is called when switching to pane view to initialize the selection.
    fn select_pane_at_index(&mut self) {
        for (i, flat) in self.all_panes.iter().enumerate() {
            // Find the first pane that matches the current input and is at the result_index
            if self.is_match(flat.title.as_str()) && i == self.result_index {
                self.pane_match = Some(flat.pane_id);
                self.pane_title_match = flat.title.clone();
                self.pane_tab_position = Some(flat.tab_position);
                break;
            }
        }
    }

    /// Move the pane selection down (to the next matching pane in the list).
    /// Wraps around to the first match if we reach the end.
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

    /// Move the pane selection up (to the previous matching pane in the list).
    /// Wraps around to the last match if we reach the beginning.
    fn move_up_pane(&mut self) {
        let mut prev_match = None;
        let mut last_match = None;

        // Reset current selection before searching
        self.pane_match = None;
        self.pane_title_match = String::default();
        self.pane_tab_position = None;

        for (i, flat) in self.all_panes.iter().enumerate() {
            if self.is_match(flat.title.as_str()) {
                // If we've reached the current selection and have a previous match, use it
                if i == self.result_index && prev_match.is_some() {
                    break;
                }
                prev_match = Some(i);
                last_match = Some(i);
            }
        }

        // Use the previous match if found
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

    /// remove_input_at_index  removes char at the
    /// cursor index and update input.
    /// Returns true if the input has change
    fn remove_input_at_index(&mut self) -> bool {
        if self.input.is_empty() {
            self.input.pop();
        } else if self.input_cusror_index > 0 && self.input_cusror_index <= self.input.len() {
            self.input.remove(self.input_cusror_index - 1);
            // update cursor index
            self.input_cusror_index -= 1;

            return true;
        } else if self.input_cusror_index == 0 {
            self.input.remove(0);
        }
        false
    }

    /// remove_input_at_index  removes char at the
    /// cursor index and update input.
    /// Returns true if the input has change
    fn insert_input_at_index(&mut self, c: char) -> bool {
        if self.input.is_empty() {
            self.input.push(c);

            // update cursor index
            self.input_cusror_index += 1;
            return true;
        } else if self.input_cusror_index > 0 && self.input_cusror_index <= self.input.len() {
            self.input.insert(self.input_cusror_index, c);
            // update cursor index
            self.input_cusror_index += 1;

            return true;
        } else if self.input_cusror_index == 0 {
            self.input.insert(0, c);
            self.input_cusror_index += 1;
        }
        false
    }

    /// print the input prompt
    fn print_prompt(&self, _rows: usize, _cols: usize) {
        // if not enough space in UI
        // input prompt
        let prompt = " > ".cyan().bold().to_string();
        if self.input.is_empty() {
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

    fn print_non_empty_input_prompt(&self, prompt: String) {
        match self.input_cusror_index.cmp(&self.input.len()) {
            std::cmp::Ordering::Equal => {
                println!("{} {}{}", prompt, self.input.dimmed(), "┃".bold().white(),);
            }
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

            std::cmp::Ordering::Greater => (),
        }
    }
}

register_plugin!(State);
impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.userspace_configuration = configuration;

        // Permission
        // - ReadApplicationState => for Tab and Pane update
        // - ChangeApplicationState => rename plugin pane, close managed paned
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
        subscribe(&[
            EventType::ModeUpdate,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::Key,
            EventType::SessionUpdate,
        ]);

        rename_plugin_pane(get_plugin_ids().plugin_id, "PathFinder");
    }

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
                self.sessions = session_infos
                    .into_iter()
                    .map(|session_info| session_info.name)
                    .collect();
                // self.sessions = session_infos;
            }

            Event::Key(key) => {
                should_render = self.handle_key_event(key);
            }
            _ => (),
        };

        should_render
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // get the shell args from config

        let debug = self.userspace_configuration.get("debug");
        // count keep tracks of lines printed
        // 4 lines for CWD and keybinding views
        let mut count = 4;

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

        self.print_prompt(rows, cols);
        count += 1;

        match self.current_view {
            View::Tab => {
                println!("Tabs: ");

                count += 1;

                for (i, t) in self.tab_infos.iter().enumerate() {
                    if self.is_match(t.name.as_str()) {
                        // limits display of completion
                        // based on available rows in pane
                        // with arbitrary buffer for safety
                        if count >= rows - 4 {
                            println!(" - {}", "...".dimmed());
                            break;
                        }

                        if i == self.result_index {
                            println!(" - {}", t.name.blue().bold());
                        } else {
                            println!(" - {}", t.name.dimmed());
                        }

                        count += 1;
                    }
                }
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
                // Display ALL panes across ALL tabs (not just one tab)
                println!("Panes (all tabs): ");

                // Iterate through the flattened list of all panes
                for (i, flat) in self.all_panes.iter().enumerate() {
                    if self.is_match(flat.title.as_str()) {
                        // Limit display based on available rows in the plugin pane
                        if count >= rows - 4 {
                            println!(" - {}", "...".dimmed());
                            break;
                        }

                        // Show pane title with its tab name in brackets so the
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

                println!();

                // Show which pane is currently selected
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
                for (i, session) in self.sessions.iter().enumerate() {
                    if self.is_match(session) {
                        // limits display of completion
                        // based on available rows in pane
                        // with arbitrary buffer for safety
                        if count >= rows - 4 {
                            println!(" - {}", "...".dimmed());
                            break;
                        }

                        if i == self.result_index {
                            println!(" - {}", session.blue().bold());
                        } else {
                            println!(" - {}", session.dimmed());
                        }

                        count += 1;
                    }
                }

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

        // Key binding view

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

pub const CYAN: u8 = 51;
pub const GRAY_LIGHT: u8 = 238;
pub const GRAY_DARK: u8 = 245;
pub const WHITE: u8 = 15;
pub const BLACK: u8 = 16;
pub const RED: u8 = 124;
pub const GREEN: u8 = 154;
pub const ORANGE: u8 = 166;

fn color_bold(color: u8, text: &str) -> String {
    format!("{}", Style::new().fg(Fixed(color)).bold().paint(text))
}
