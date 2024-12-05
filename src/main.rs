use bytesize::ByteSize;
use rayon::prelude::*;
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
    let mut cwd = Path::new("/home/kjc/Downloads");
    let now = Instant::now();
    let tree = scan(cwd);
    let elapsed = now.elapsed();

    let mut terminal = ratatui::init();

    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let mut items: Vec<_> = tree
        .find(cwd)
        .children
        .iter()
        .map(|(k, v)| (k.to_str().unwrap(), v.info.clone()))
        .collect();
    items.sort_by(|(_, a), (_, b)| b.size.cmp(&a.size));

    loop {
        terminal
            .draw(|frame| {
                let list = List::new(
                    items
                        .clone()
                        .into_iter()
                        .map(|(k, i)| format!("{} {:?}", ByteSize(i.size), k)),
                )
                .block(Block::bordered().title(format!(
                    "Files - {:?} {} ({:.0?})",
                    cwd.file_name().unwrap(),
                    items.len(),
                    elapsed
                )))
                .style(Style::new().white())
                .highlight_style(Style::new().italic())
                .highlight_symbol("> ")
                .repeat_highlight_symbol(true)
                .direction(ListDirection::TopToBottom);

                frame.render_stateful_widget(list, frame.area(), &mut list_state);
            })
            .expect("failed to draw frame");

        if let Event::Key(key) = event::read().unwrap() {
            match key.code {
                KeyCode::Char('k') => list_state.select_previous(),
                KeyCode::Char('j') => list_state.select_next(),
                KeyCode::Char('q') | KeyCode::Esc => break,
                _ => {}
            }
        }
    }
    ratatui::restore();
}
