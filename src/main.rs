use bytesize::ByteSize;
use crossterm::event::{self, Event, KeyCode, MouseEvent, MouseEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::CrosstermBackend;
use ratatui::style::{Color, Modifier};
use ratatui::text::Span;
use ratatui::Terminal;
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    widgets::{Block, List, ListDirection, ListItem, ListState},
};
use rayon::prelude::*;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone)]
struct Info {
    path: PathBuf,
    size: u64,
    is_dir: bool,
}

#[derive(Debug, Clone)]
struct Tree {
    data: Vec<Info>,
}

impl Tree {
    fn accumulate(self: &mut Self) {
        let mut sums: [u64; 4096] = [0; 4096];
        let mut prev_depth = 0;
        for i in (0..self.data.len()).rev() {
            let depth = self.data[i].path.components().count();
            if depth < prev_depth {
                self.data[i].size += sums[prev_depth];
                sums[prev_depth] = 0;
            }
            sums[depth] += self.data[i].size;
            prev_depth = depth;
        }
    }

    fn get(self: &Self, p: &Path) -> Vec<Info> {
        let start = self
            .data
            .binary_search_by(|x| x.path.cmp(&p.to_path_buf()))
            .unwrap();
        let end = self.data[start..]
            .into_iter()
            .position(|x| !x.path.starts_with(&p.to_path_buf()))
            .unwrap_or(self.data.len());

        let mut items: Vec<Info> = self.data[start..start + end]
            .iter()
            .filter(|x| x.path.components().count() == p.components().count() + 1)
            .cloned()
            .collect();
        items.sort_by(|a, b| b.size.cmp(&a.size));
        items
    }
}

fn scan(folder: &Path) -> Tree {
    let data: Vec<_> = jwalk::WalkDir::new(folder)
        .sort(true)
        .into_iter()
        .filter_map(Result::ok)
        .map(|x| {
            let metadata = x.metadata().unwrap();
            Info {
                path: x.path().to_path_buf(),
                size: metadata.size(),
                is_dir: metadata.is_dir(),
            }
        })
        .collect();
    Tree { data }
}

fn main() {
    // let mut cwd = Path::new("/home/kjc/Downloads").to_path_buf();
    let mut cwd = Path::new("/home/kjc/closet").to_path_buf();
    let mut depths = vec![0];
    let now = Instant::now();
    let mut tree = scan(&cwd);
    tree.accumulate();
    let elapsed = now.elapsed();

    enable_raw_mode().unwrap();
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen).unwrap();
    crossterm::execute!(stdout, crossterm::event::EnableMouseCapture).unwrap();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut list_state = ListState::default();
    let mut list_area = Rect::default();
    list_state.select(Some(0));

    let mut items: Vec<_> = tree.get(&cwd);
    loop {
        terminal
            .draw(|frame| {
                list_area = frame.area();
                let list = List::new(items.clone().into_iter().map(|i| {
                    ListItem::new(Span::styled(
                        format!("{:>8} {:?}", ByteSize(i.size), i.path.file_name().unwrap()),
                        Style::default().fg(if i.is_dir { Color::Blue } else { Color::White }),
                    ))
                }))
                .block(Block::bordered().title(format!(
                    "Files - {:?} {} ({:.0?})",
                    cwd.file_name().unwrap(),
                    items.len(),
                    elapsed
                )))
                .style(Style::new().white())
                .highlight_style(
                    Style::default()
                        .bg(Color::Yellow)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> ")
                .repeat_highlight_symbol(true)
                .direction(ListDirection::TopToBottom);

                frame.render_stateful_widget(list, frame.area(), &mut list_state);
            })
            .expect("failed to draw frame");

        match event::read().unwrap() {
            Event::Key(key) => match key.code {
                KeyCode::Char('k') => list_state.select_previous(),
                KeyCode::Char('j') => list_state.select_next(),
                KeyCode::Char('G') => list_state.select_last(),
                KeyCode::Char('g') => list_state.select_first(),
                KeyCode::Char('-') => {
                    if depths.len() >= 2 {
                        cwd.pop();
                        items = tree.get(&cwd);
                        list_state.select(Some(depths.pop().unwrap()));
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Enter => {
                    if let Some(selected) = list_state.selected() {
                        let i = &items[selected];
                        if i.is_dir {
                            cwd = i.path.clone();
                            depths.push(selected);
                            items = tree.get(&cwd);
                        } else {
                            Command::new("xdg-open")
                                .arg(i.path.clone())
                                .spawn()
                                .unwrap();
                        }
                    }
                }
                _ => {}
            },
            Event::Mouse(MouseEvent { kind, row, .. }) => match kind {
                MouseEventKind::Down(_) => {
                    if row >= list_area.y && row < list_area.y + list_area.height {
                        let index = (row - list_area.y - 1) as usize;
                        if index < items.len() {
                            list_state.select(Some(index));
                        }
                    }
                }
                _ => {}
            },
            _ => continue,
        }
    }

    disable_raw_mode().unwrap();
    crossterm::execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )
    .unwrap();
}
