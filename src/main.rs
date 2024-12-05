use bytesize::ByteSize;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::symbols::scrollbar;
use ratatui::text::Span;
use ratatui::widgets::Scrollbar;
use ratatui::widgets::ScrollbarOrientation;
use ratatui::widgets::ScrollbarState;
use rayon::prelude::*;
use std::alloc::Layout;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::Component;
use std::path::Components;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;
use std::{collections::HashMap, time::Duration};
use walkdir::DirEntry;

use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    widgets::{Block, List, ListDirection, ListItem, ListState},
    Frame,
};

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
    fn insert(
        self: &mut Self,
        metadata: Metadata,
        components: &mut impl Iterator<Item = OsString>,
    ) {
        if let Some(segment) = components.next() {
            if !self.children.contains_key(&segment) {
                self.children.insert(
                    segment.clone(),
                    Tree {
                        info: Info::new(),
                        children: HashMap::new(),
                    },
                );
            }
            self.children
                .entry(segment)
                .and_modify(|t| t.insert(metadata, components));
        } else {
            self.info = Info {
                size: metadata.size(),
                is_dir: metadata.is_dir(),
            };
        }
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
        tree.insert(
            p.metadata().unwrap(),
            &mut p
                .path()
                .components()
                .skip(1)
                .map(|x| x.as_os_str().to_os_string()),
        );
    }
    tree.init();
    tree
}

fn main() {
    let mut cwd = Path::new("/home/kjc/Downloads").to_path_buf();
    let mut depth = 0;
    let now = Instant::now();
    let tree = scan(&cwd);
    let elapsed = now.elapsed();

    let mut terminal = ratatui::init();

    let mut list_state = ListState::default();
    let mut scroll_state = ScrollbarState::default();
    list_state.select(Some(0));

    let mut items: Vec<_> = tree.get(&cwd);
    loop {
        terminal
            .draw(|frame| {
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

                let scrollbar = Scrollbar::default()
                    .orientation(ScrollbarOrientation::VerticalRight)
                    .symbols(scrollbar::VERTICAL)
                    .begin_symbol(None)
                    .end_symbol(None);

                frame.render_stateful_widget(list, frame.area(), &mut list_state);
            })
            .expect("failed to draw frame");

        if let Event::Key(key) = event::read().unwrap() {
            match key.code {
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
            }
        }
    }
    ratatui::restore();
}
