//! The sidebar: projects, their sessions, and the lists that open in it.

use super::{HEADER, Hit, Hits, INSET, ROW, Ui, chrome, field, picker, theme};
use crate::action::Action;
use crate::core::{Doing, Project, Session, SessionId, Workspace};
use crate::render::font::{BOLD, UI};
use crate::render::{Canvas, Icon, Rect};

/// The room below the rows of a project, before the next one.
const GAP: f32 = 8.0;
/// The height of a line of the notice.
const NOTICE_LINE: f32 = 19.0;
/// Where the mark begins that says what a session is doing, and the
/// folder of a project above it: one column, in the middle of the strip
/// the sidebar folds to.
const STATE: f32 = 14.0;
/// Where the name of a session begins, behind that mark, and the name of
/// a project behind its folder.
const NAME: f32 = 36.0;
/// The height of a row below that of a session: one of the things its
/// program runs beside itself, an agent it sent off.
const SUB: f32 = 24.0;
/// The size of the mark in such a row, and the room it has with what
/// follows it. A row that lies below another row begins that much further
/// in, up to `SUB_DEPTH` times.
const SUB_MARK: f32 = 12.0;
const SUB_STEP: f32 = 18.0;
const SUB_DEPTH: usize = 2;
/// The arrow that leads to such a row from what it hangs on: the room
/// between its tip and the mark of the row, and where its tip lies on
/// the sixteen it is drawn on.
const ARROW_GAP: f32 = 3.0;
const ARROW_TIP: f32 = 14.0;

/// The middle of that column.
fn column(ui: &Ui) -> f32 {
    ui.px(STATE) + ui.px(chrome::ICON) / 2.0
}

/// The row at the bottom while a list is open: its text field.
pub fn field_row(ui: &Ui) -> Rect {
    let area = ui.sidebar();
    let row_h = ui.px(ROW);
    Rect::new(ui.px(8.0), area.h - row_h - ui.px(10.0), area.w - ui.px(16.0), row_h)
}

/// The height of the rows below that of a session: one for each thing
/// its program runs beside itself that is shown, and one that says how
/// many more there are. None in the strip of a folded sidebar, which has
/// the mark of the session alone.
fn subs_height(ui: &Ui, session: &Session) -> f32 {
    let (shown, more) = session.sub_rows();
    if ui.rail() { 0.0 } else { (shown.len() + usize::from(more > 0)) as f32 * ui.px(SUB) }
}

/// The height of the rows of a project: its own, and those of its sessions.
fn project_height(ui: &Ui, sessions: &[&Session]) -> f32 {
    let sessions = sessions.iter().map(|session| ui.px(ROW) + subs_height(ui, session));
    ui.px(ROW) + sessions.sum::<f32>()
}

/// Scrolls the list so far that the row of a session is in it. A field
/// that opens in a row nobody sees would take the keyboard unnoticed.
pub fn reveal(ui: &mut Ui, ws: &Workspace, id: SessionId) {
    let (row_h, mut y) = (ui.px(ROW), 0.0);
    for project in &ws.projects {
        y += row_h;
        for session in ws.sessions_of(project.id) {
            if session.id == id {
                let view = ui.sidebar_bottom - ui.px(HEADER);
                let least = (y + row_h - view).max(0.0).min(y);
                ui.sidebar_scroll = ui.sidebar_scroll.clamp(least, y);
                return;
            }
            y += row_h + subs_height(ui, session);
        }
        y += ui.px(GAP);
    }
}

/// The room a notice of this many lines takes at the bottom of the list.
fn notice_height(ui: &Ui, lines: usize) -> f32 {
    if lines == 0 { 0.0 } else { lines as f32 * ui.px(NOTICE_LINE) + ui.px(8.0) }
}

/// Draws the sidebar: the rows of the list first, then what covers the ones
/// scrolled out of it, then what lies on top.
pub fn draw(cv: &mut Canvas, ui: &mut Ui, ws: &Workspace, hits: &mut Hits) {
    let area = ui.sidebar();
    let rail = ui.rail();
    let (row_h, px, inset) = (ui.px(ROW), ui.text_px, ui.px(INSET));
    let top = ui.px(HEADER);
    let field = field_row(ui);
    let front = ws.focused_session();
    let notice = ui.notice.as_ref().filter(|_| ui.picker.is_none());
    let lines = notice.map_or_else(Vec::new, |text| wrap(cv, ui, text, area.w - 2.0 * inset, 3));
    let line_h = ui.px(NOTICE_LINE);
    let notice_h = notice_height(ui, lines.len());
    // The rows go down to the bar, unless a list with its field is open.
    let below = if ui.picker.is_some() { field.y } else { area.h };
    ui.sidebar_bottom = below - notice_h;
    // Every session is looked at once for a frame, however many
    // projects there are, and what is not in the list is passed by its
    // height alone.
    let groups = ws.grouped();
    ui.sidebar_height = groups.iter().map(|(_, sessions)| project_height(ui, sessions) + ui.px(GAP)).sum();
    clamp(ui);

    cv.rect(area, theme::SIDEBAR);
    cv.rect(Rect::new(area.w, 0.0, 1.0, area.h), theme::LINE);
    // Folded and drawn whole for a list, it lies over a pane: what is
    // under it is not to be clicked through it.
    hits.push((area, Hit::None));

    let start = top - ui.sidebar_scroll;
    let mut y = start;
    // While the column with the files is there, the row of a project
    // shows its files in it: the way to a project without a session.
    let files = ui.tree(ws.panes().len()).is_some();
    for (project, sessions) in &groups {
        // Rows scrolled out of the list are covered anyway, by the
        // header and by what lies below. They are not named, measured
        // or drawn, and nothing of them can be clicked.
        let seen = |y: f32, height: f32| y + height > top && y < area.h;
        if seen(y, row_h) {
            if files {
                let (row, show) = (Rect::new(0.0, y, area.w, row_h), Action::TreeOf(project.id));
                if ui.hot(show) && ws.tree.project != Some(project.id) {
                    cv.rect(row, theme::HOVER);
                }
                hits.push((row, Hit::Do(show)));
            }
            project_row(cv, ui, project, sessions.is_empty(), rail, y, hits);
        }
        y += row_h;
        // Which sessions of the project read alike is found once, when
        // the first of its rows is in the list.
        let mut earlier = None;
        for (place, session) in sessions.iter().enumerate() {
            let height = row_h + subs_height(ui, session);
            if seen(y, height) {
                let earlier = earlier.get_or_insert_with(|| Workspace::earlier(project, sessions))[place];
                session_row(cv, ui, ws, session, earlier, front, rail, y, hits);
            }
            y += height;
        }
        y += ui.px(GAP);
    }
    let height = y - start;

    // Something went wrong that the user should know of. It takes as many
    // lines as it needs, up to three. A click hides it.
    // Rows scrolled out of the list must not show through header and footer.
    let footer = Rect::new(0.0, below - notice_h, area.w, area.h - below + notice_h);
    cv.rect(Rect::new(0.0, 0.0, area.w, top), theme::SIDEBAR);
    cv.rect(footer, theme::SIDEBAR);
    hits.push((Rect::new(0.0, 0.0, area.w, top), Hit::Drag));
    hits.push((footer, Hit::None));
    // Above the column of the folders and the marks, which is the
    // middle of the strip the sidebar folds to.
    chrome::logo(cv, ui, column(ui), top);
    for (n, line) in lines.iter().enumerate() {
        let row = Rect::new(0.0, footer.y + n as f32 * line_h, area.w, line_h);
        cv.text(line, UI, px, inset, ui.baseline(row), theme::DANGER, area.w - 2.0 * inset);
    }
    if notice_h > 0.0 {
        hits.push((Rect::new(0.0, footer.y, area.w, notice_h), Hit::Do(Action::DismissNotice)));
    }
    if let Some(open) = &ui.picker {
        picker::draw(cv, ui, open, hits);
    }
    // The height as the rows added up to it, which is what the wheel is
    // held to until the next frame. Not the sum from above: with the list
    // scrolled by part of a row the two can differ in the last place.
    ui.sidebar_height = height;
}

/// The row of a project: the name, at the right the git state, and for a
/// project without sessions a button to remove it, left of that. In the
/// strip of a folded sidebar, `rail`, the folder alone.
fn project_row(
    cv: &mut Canvas,
    ui: &Ui,
    project: &Project,
    empty: bool,
    rail: bool,
    y: f32,
    hits: &mut Hits,
) {
    let area = ui.sidebar();
    let (row_h, px, inset) = (ui.px(ROW), ui.text_px, ui.px(INSET));
    let icon = ui.px(chrome::ICON);
    // Above the marks of its sessions, in their column.
    chrome::symbol(cv, ui, Icon::Folder, Rect::new(ui.px(STATE), y, icon, row_h), theme::DIM);
    if rail {
        return;
    }
    let pid = project.id;
    let row = Rect::new(0.0, y, area.w, row_h);
    let base = ui.baseline(row);
    // At the right: what starts a session here, with an agent or from
    // an earlier conversation. Always there, so nothing in the list
    // moves when the pointer does.
    let mut right = area.w - inset;
    if ui.agents.iter().any(|agent| agent.installed) {
        let button = Rect::new(area.w - ui.px(8.0) - icon - ui.px(6.0), y, icon + ui.px(12.0), row_h);
        let launch = Action::Launch(pid);
        chrome::symbol(cv, ui, Icon::Plus, button, ui.lit(launch, theme::DIM));
        hits.push((button, Hit::Do(launch)));
        right = button.x - ui.px(2.0);
    }
    if let Some(git) = &project.git {
        let diff = Action::ToggleDiff(pid);
        let color = ui.lit(diff, theme::DIM);
        let label =
            if git.changed > 0 { format!("{}  {}", git.branch, git.changed) } else { git.branch.clone() };
        let max = area.w * 0.4;
        let width = cv.text_width(&label, UI, px).min(max);
        cv.text(&label, UI, px, right - width, base, color, max);
        let mark = Rect::new(right - width - ui.px(3.0) - icon, y, icon, row_h);
        chrome::symbol(cv, ui, Icon::GitBranch, mark, color);
        let target = Rect::new(mark.x - ui.px(4.0), y, right - mark.x + ui.px(4.0), row_h);
        hits.push((target, Hit::Do(diff)));
        right = target.x;
    }
    if empty {
        let button = Rect::new(right - ui.px(22.0), y, ui.px(22.0), row_h);
        chrome::close_button(cv, ui, button, 8.0, theme::FAINT, Action::RemoveProject(pid), hits);
        right = button.x;
    }
    let name = ui.px(NAME);
    cv.text(&project.name, UI | BOLD, px, name, base, theme::TEXT, right - name - ui.px(4.0));
}

/// The row of a session: the mark of its state, its name or the
/// field that renames it, and while the pointer is on it what closes it.
/// Below it the rows of what its program runs beside itself. In the strip
/// of a folded sidebar, `rail`, that mark alone.
#[allow(clippy::too_many_arguments)]
fn session_row(
    cv: &mut Canvas,
    ui: &Ui,
    ws: &Workspace,
    session: &Session,
    earlier: usize,
    front: Option<SessionId>,
    rail: bool,
    y: f32,
    hits: &mut Hits,
) {
    let area = ui.sidebar();
    let (row_h, px) = (ui.px(ROW), ui.text_px);
    let (inset, title_x) = (ui.px(INSET), ui.px(NAME));
    let row = Rect::new(0.0, y, area.w, row_h);
    // With the rows below it, which are of this session and open it.
    let all = Rect::new(0.0, y, area.w, row_h + subs_height(ui, session));
    let (open, close) = (Action::Open(session.id), Action::CloseSession(session.id));
    let hovered = ui.hot(open) || ui.hot(close) || ui.hot(Action::OpenBelow(session.id));
    if front == Some(session.id) {
        cv.rect(all, theme::ACTIVE);
    } else if hovered || ws.visible(session.id) {
        cv.rect(all, theme::HOVER);
    }
    session_hits(ui, session, y, hits);

    // Its state, and nothing of its program: which program a session
    // runs is said once, in the header of the pane that shows it. Here
    // it stood before every name, the same mark ten times over, and
    // took the eye off the state.
    chrome::state(cv, ui, session, Rect::new(ui.px(STATE), y, ui.px(chrome::ICON), row_h));
    if rail {
        return;
    }
    sub_rows(cv, ui, session, row.bottom());

    let mut right = area.w - inset;
    if hovered {
        let button = Rect::new(area.w - row_h - ui.px(4.0), y, row_h, row_h);
        chrome::close_button(cv, ui, button, 9.0, theme::DIM, close, hits);
        right = button.x;
    }
    let base = ui.baseline(row);

    match &ui.renaming {
        Some((id, text)) if *id == session.id => {
            let field = Rect::new(
                title_x - ui.px(6.0),
                y + ui.px(3.0),
                right - title_x + ui.px(6.0),
                row_h - ui.px(6.0),
            );
            cv.rounded(field, ui.px(5.0), theme::BG);
            hits.push((field, Hit::Field));
            field::draw(cv, ui, field, title_x, text, "", theme::TEXT);
        }
        _ => {
            let started = session.term.is_some() || front == Some(session.id);
            let color = if started { theme::TEXT } else { theme::DIM };
            cv.text(&ws.numbered(session, earlier), UI, px, title_x, base, color, right - title_x);
        }
    }
}

/// What a click does on the row of a session at `y`, and on the rows
/// below it: both open the session. They are told apart because a second
/// click renames a session, and the field for that opens in its own row.
/// With a row below it clicked, that row can lie above the list, and the
/// field took the keyboard where nobody saw it.
fn session_hits(ui: &Ui, session: &Session, y: f32, hits: &mut Hits) {
    let (width, row_h, below) = (ui.sidebar().w, ui.px(ROW), subs_height(ui, session));
    hits.push((Rect::new(0.0, y, width, row_h), Hit::Do(Action::Open(session.id))));
    if below > 0.0 {
        hits.push((Rect::new(0.0, y + row_h, width, below), Hit::Do(Action::OpenBelow(session.id))));
    }
}

/// Where the mark of a row below a session begins that lies this deep.
fn sub_mark(ui: &Ui, depth: usize) -> f32 {
    ui.px(NAME) + depth as f32 * ui.px(SUB_STEP)
}

/// Where the arrow begins that leads to such a row: so far before the
/// mark of the row that its tip leaves a little room to it. Its stroke
/// down then stands below the mark the row hangs on, that of the
/// session, or of the row above for a row that lies below a row.
fn arrow(ui: &Ui, depth: usize) -> f32 {
    sub_mark(ui, depth) - ui.px(ARROW_GAP) - ui.px(ARROW_TIP)
}

/// Draws the arrow that leads to a row below a session: it comes from
/// above and turns to the row.
fn branch(cv: &mut Canvas, ui: &Ui, depth: usize, row: Rect) {
    let area = Rect::new(arrow(ui, depth), row.y, ui.px(chrome::ICON), row.h);
    chrome::symbol(cv, ui, Icon::Indent, area, theme::FAINT);
}

/// The rows below that of a session, from `y` down: what its program
/// runs beside itself, each with the mark of its state and what it is
/// doing, in the program's words: what the program reports of it in its
/// file where it does, else the title it told the terminal. An arrow
/// leads to each from what it hangs on. They are not rows to act on: a
/// click on one opens the session, as one on the row above does.
fn sub_rows(cv: &mut Canvas, ui: &Ui, session: &Session, mut y: f32) {
    let area = ui.sidebar();
    let (row_h, px, right) = (ui.px(SUB), ui.text_px, area.w - ui.px(INSET));
    let (shown, more) = session.sub_rows();
    for sub in shown {
        let (row, depth) = (Rect::new(0.0, y, area.w, row_h), sub.depth.min(SUB_DEPTH));
        branch(cv, ui, depth, row);
        let x = sub_mark(ui, depth);
        chrome::mark(cv, ui, Doing::said(sub.state), Rect::new(x, y, ui.px(SUB_MARK), row_h), SUB_MARK);
        let title_x = x + ui.px(SUB_STEP);
        cv.text(session.doing_of(sub), UI, px, title_x, ui.baseline(row), theme::DIM, right - title_x);
        y += row_h;
    }
    if more > 0 {
        let (row, x) = (Rect::new(0.0, y, area.w, row_h), sub_mark(ui, 0));
        branch(cv, ui, 0, row);
        cv.text(&format!("+ {more} more"), UI, px, x, ui.baseline(row), theme::FAINT, right - x);
    }
}

/// Breaks text into at most `max` lines of the given width, at spaces.
fn wrap(cv: &mut Canvas, ui: &Ui, text: &str, width: f32, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let fits = lines
            .last()
            .is_some_and(|line| cv.text_width(&format!("{line} {word}"), UI, ui.text_px) <= width);
        let full = lines.len() == max;
        match lines.last_mut() {
            Some(line) if fits || full => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// Keeps the scroll offset of the list within it.
pub fn clamp(ui: &mut Ui) {
    ui.sidebar_scroll = ui.sidebar_scroll.clamp(0.0, max_scroll(ui));
}

/// Scrolls the list by `lines` of the wheel, positive is up.
pub fn wheel(ui: &mut Ui, lines: i32) {
    ui.sidebar_scroll -= lines as f32 * ui.px(ROW) / 3.0;
    clamp(ui);
}

/// How far the project list can be scrolled.
pub fn max_scroll(ui: &Ui) -> f32 {
    let bottom = if ui.sidebar_bottom > 0.0 { ui.sidebar_bottom } else { ui.sidebar().h };
    (ui.sidebar_height - (bottom - ui.px(HEADER)).max(0.0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_out_of_view_is_scrolled_into_it() {
        let mut ws = Workspace::restore(&Default::default());
        let project = ws.add_project("/p/a".into());
        let ids: Vec<SessionId> = (0..40).map(|_| ws.new_session(project, "shell")).collect();
        let mut ui = Ui::new(Vec::new());
        ui.height = ui.px(320.0);
        // The list ends at the bar.
        ui.sidebar_bottom = ui.sidebar().h;
        assert_eq!(ui.sidebar_bottom, ui.px(320.0) - ui.px(super::super::BAR));
        let (row, view) = (ui.px(ROW), ui.sidebar_bottom - ui.px(HEADER));
        // The project's own row lies above its sessions.
        let top = |ui: &Ui, n: usize| (n + 1) as f32 * row - ui.sidebar_scroll;
        reveal(&mut ui, &ws, ids[30]);
        assert!(top(&ui, 30) >= 0.0 && top(&ui, 30) + row <= view);
        reveal(&mut ui, &ws, ids[2]);
        assert!(top(&ui, 2) >= 0.0 && top(&ui, 2) + row <= view);
        // A row that is in view moves nothing.
        let before = ui.sidebar_scroll;
        reveal(&mut ui, &ws, ids[3]);
        assert_eq!(ui.sidebar_scroll, before);
    }

    #[test]
    fn what_a_session_runs_beside_itself_has_rows_below_it_but_not_in_the_strip() {
        use crate::core::program::{State, Sub};
        let mut ws = Workspace::restore(&Default::default());
        let project = ws.add_project("/p/a".into());
        let ids: Vec<SessionId> = (0..40).map(|_| ws.new_session(project, "shell")).collect();
        let mut ui = Ui::new(Vec::new());
        ui.height = ui.px(320.0);
        ui.sidebar_bottom = ui.sidebar().h;
        let (row, sub, view) = (ui.px(ROW), ui.px(SUB), ui.sidebar_bottom - ui.px(HEADER));
        let plain = project_height(&ui, &ws.grouped()[0].1);
        assert_eq!(plain, 41.0 * row);
        let subs =
            |n: usize| vec![Sub { id: "a".into(), state: State::Working, title: "one".into(), depth: 0 }; n];
        ws.session_mut(ids[1]).unwrap().runs(subs(3));
        // More than are shown: the rows there are, and one for the rest.
        ws.session_mut(ids[4]).unwrap().runs(subs(20));
        let shown = ws.session(ids[4]).unwrap().sub_rows().0.len();
        assert!(shown < 20);
        let rows = 3 + shown + 1;
        assert_eq!(project_height(&ui, &ws.grouped()[0].1), plain + rows as f32 * sub);
        // A row further down lies that much lower, and is scrolled to there.
        reveal(&mut ui, &ws, ids[30]);
        let top = 31.0 * row + rows as f32 * sub - ui.sidebar_scroll;
        assert!(top >= 0.0 && top + row <= view, "{top}");
        // The strip has the marks of the sessions alone.
        ui.sidebar_folded = true;
        assert!(ui.rail());
        assert_eq!(project_height(&ui, &ws.grouped()[0].1), plain);
    }

    #[test]
    fn a_row_below_a_session_opens_it_and_is_not_the_row_that_renames_it() {
        use crate::core::program::{State, Sub};
        let mut ws = Workspace::restore(&Default::default());
        let project = ws.add_project("/p/a".into());
        let id = ws.new_session(project, "shell");
        let ui = Ui::new(Vec::new());
        let subs = vec![Sub { id: "a".into(), state: State::Working, title: "one".into(), depth: 0 }; 3];
        ws.session_mut(id).unwrap().runs(subs);
        let (session, y) = (ws.session(id).unwrap(), ui.px(HEADER + ROW));
        let mut hits = Hits::new();
        session_hits(&ui, session, y, &mut hits);
        let at =
            |y: f32| hits.iter().rev().find(|(area, _)| area.contains(ui.px(100.0), y)).map(|(_, hit)| *hit);
        // The row of the session itself: a second click on it renames it,
        // which is what `Action::Open` under two clicks means.
        assert_eq!(at(y + ui.px(ROW) / 2.0), Some(Hit::Do(Action::Open(id))));
        // A row below it opens the session and no more. The field that
        // renames would open in the row above, which can lie out of the
        // list while this one is clicked, and take the keyboard unseen.
        for row in 0..3 {
            let middle = y + ui.px(ROW) + (row as f32 + 0.5) * ui.px(SUB);
            assert_eq!(at(middle), Some(Hit::Do(Action::OpenBelow(id))), "row {row}");
        }
        assert_eq!(at(y + ui.px(ROW) + 3.0 * ui.px(SUB)), None, "nothing below the last");
        // A session without such rows has the one hit.
        ws.session_mut(id).unwrap().runs(Vec::new());
        hits.clear();
        session_hits(&ui, ws.session(id).unwrap(), y, &mut hits);
        assert_eq!(hits.len(), 1);
    }

    /// Where the stroke down of the arrow lies on the sixteen it is
    /// drawn on.
    const STROKE: f32 = 2.5;

    #[test]
    fn the_arrow_to_a_row_below_a_session_comes_from_the_mark_the_row_hangs_on() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut ui = Ui::new(Vec::new());
            ui.scale = scale;
            for depth in 0..=SUB_DEPTH {
                let (from, mark) = (arrow(&ui, depth), sub_mark(&ui, depth));
                // Its tip ends before the mark of its row, with room
                // between them, and it begins within the sidebar.
                let tip = from + ui.px(ARROW_TIP);
                assert!(from >= 0.0 && tip < mark && mark - tip >= 2.0, "depth {depth} at {scale}");
                // Its stroke down stands below the mark the row hangs
                // on: that of the session, or of the row above.
                let stroke = from + ui.px(STROKE);
                let (left, width) = match depth {
                    0 => (ui.px(STATE), ui.px(chrome::ICON)),
                    _ => (sub_mark(&ui, depth - 1), ui.px(SUB_MARK)),
                };
                assert!(stroke >= left && stroke <= left + width, "depth {depth} at {scale}: {stroke}");
            }
        }
    }

    #[test]
    fn the_marks_of_a_folded_sidebar_stand_in_the_middle_of_its_strip() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut ui = Ui::new(Vec::new());
            ui.scale = scale;
            // The folder of a project and the marks of its sessions are
            // drawn at the same place, a square of an icon.
            let middle = column(&ui);
            assert!((middle - ui.px(crate::ui::RAIL) / 2.0).abs() <= 1.0, "{middle} at {scale}");
            // The mark of codn stands above them, and within the strip.
            let (from, width) = (chrome::logo_x(&ui, middle), chrome::logo_width(&ui));
            assert!((from + width / 2.0 - middle).abs() <= 0.5, "the mark at {scale}");
            assert!(from >= 0.0 && from + width <= ui.px(crate::ui::RAIL));
            // And a name begins behind them.
            assert!(ui.px(NAME) >= ui.px(STATE) + ui.px(chrome::ICON));
        }
    }

    #[test]
    fn scrolling_reveals_last_rows_above_notices_at_each_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut ui = Ui::new(Vec::new());
            ui.scale = scale;
            ui.height = ui.px(760.0);
            ui.sidebar_height = ui.px(900.0);
            for lines in [0, 1, 3] {
                ui.sidebar_bottom = ui.sidebar().h - notice_height(&ui, lines);
                let content_bottom = ui.px(HEADER) - max_scroll(&ui) + ui.sidebar_height;
                assert_eq!(content_bottom, ui.sidebar_bottom);
            }
        }
    }
}
