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
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;
use walkdir::DirEntry;

#[derive(Debug, Clone)]
struct Info {
    size: u64,
    is_dir: bool,
}

impl Info {
    fn new() -> Info {
        return Info {
            size: 0,
            is_dir: true,
        };
    }
}

#[derive(Debug, Clone)]
struct Tree {
    info: Info,
    children: HashMap<OsString, Tree>,
}

impl Tree {
    fn insert(self: &mut Self, p: &DirEntry) {
        let mut t = self;
        for segment in p.path().components().skip(1) {
            let segment = segment.as_os_str().to_os_string();
            if !t.children.contains_key(&segment) {
                t.children.insert(
                    segment.clone(),
                    Tree {
                        info: Info::new(),
                        children: HashMap::new(),
                    },
                );
            }
            t = t.children.get_mut(&segment).unwrap();
        }
        let metadata = p.metadata().unwrap();
        t.info = Info {
            size: metadata.size(),
            is_dir: metadata.is_dir(),
        };
    }

    fn init(self: &mut Self) {
        if self.info.is_dir {
            for (_, tree) in self.children.iter_mut() {
                tree.init();
            }
            self.info.size = self.children.values().map(|tree| tree.info.size).sum();
        }
    }

    fn get(self: &Self, p: &Path) -> Vec<(&str, Info)> {
        let mut items: Vec<_> = self
            .find(&p)
            .children
            .iter()
            .map(|(k, v)| (k.to_str().unwrap(), v.info.clone()))
            .collect();
        items.sort_by(|(_, a), (_, b)| b.size.cmp(&a.size));
        items
    }

    fn find(self: &Self, p: &Path) -> &Tree {
        let mut t = self;
        for segment in p.components().skip(1) {
            t = &t.children[&segment.as_os_str().to_os_string()];
        }
        return t;
    }
}

fn scan(folder: &Path) -> Tree {
    let mut tree = Tree {
        info: Info::new(),
        children: HashMap::new(),
    };
    for p in walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(Result::ok)
    {
        tree.insert(&p);
    }
    tree.init();
    tree
}

fn main() {
    let mut cwd = Path::new("/home/kjc/closet").to_path_buf();
    let mut depth = 0;
    let now = Instant::now();
    let tree = scan(&cwd);
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
                let list = List::new(items.clone().into_iter().map(|(k, i)| {
                    ListItem::new(Span::styled(
                        format!("{:>8} {:?}", ByteSize(i.size), k),
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
                    if depth - 1 >= 0 {
                        depth -= 1;
                        cwd.pop();
                        items = tree.get(&cwd);
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Enter => {
                    if let Some(selected) = list_state.selected() {
                        let (k, v) = &items[selected];
                        if v.is_dir {
                            cwd.push(k);
                            depth += 1;
                            items = tree.get(&cwd);
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
