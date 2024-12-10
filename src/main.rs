use bytesize::ByteSize;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, MouseEvent, MouseEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use jwalk::Parallelism;
use ratatui::prelude::CrosstermBackend;
use ratatui::style::{Color, Modifier};
use ratatui::text::Span;
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    widgets::{Block, List, ListDirection, ListItem, ListState},
};
use ratatui::{Frame, Terminal};
use std::cmp::Ordering;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::time::Instant;

#[derive(Debug, Clone, Default)]
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
        let end = self.data[start..].partition_point(|x| x.path.starts_with(&p.to_path_buf()));

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
    let data: Vec<_> = jwalk::WalkDirGeneric::<((), Info)>::new(folder)
        .parallelism(Parallelism::RayonNewPool(16))
        .sort(true)
        .skip_hidden(false)
        .process_read_dir(|_, _, _, dir_entry_results| {
            dir_entry_results.iter_mut().for_each(|x| match x {
                Ok(dir_entry) => {
                    let metadata = dir_entry.metadata().unwrap();
                    dir_entry.client_state = Info {
                        path: dir_entry.path().to_path_buf(),
                        size: metadata.size(),
                        is_dir: metadata.is_dir(),
                    }
                }
                Err(x) => {
                    eprintln!("Unable to index, error encountered: {:?}", x);
                    exit(1);
                }
            })
        })
        .into_iter()
        .map(|x| x.unwrap().client_state)
        .collect();
    Tree { data }
}

struct StatefulList {
    state: ListState,
    area: Rect,
    items: Vec<Info>,
}

impl StatefulList {
    fn new() -> StatefulList {
        let mut state = ListState::default();
        state.select(Some(0));
        StatefulList {
            state,
            area: Rect::default(),
            items: Vec::new(),
        }
    }

    fn render(self: &mut Self, frame: &mut Frame, status: String) {
        self.area = frame.area();
        let list = List::new(self.items.clone().into_iter().map(|i| {
            ListItem::new(Span::styled(
                format!("{:>8} {:?}", ByteSize(i.size), i.path.file_name().unwrap()),
                Style::default().fg(if i.is_dir { Color::Blue } else { Color::White }),
            ))
        }))
        .block(Block::bordered().title(status))
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

        frame.render_stateful_widget(list, frame.area(), &mut self.state);
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(default_value = ".")]
    directory: PathBuf,
    #[arg(long, short, action)]
    benchmark: bool,
}

fn main() {
    let args = Args::parse();
    let mut cwd = args.directory.canonicalize().unwrap();

    let now = Instant::now();
    let mut tree = scan(&cwd);
    tree.accumulate();
    let elapsed = now.elapsed();

    if args.benchmark {
        exit(0);
    }

    enable_raw_mode().unwrap();
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen).unwrap();
    crossterm::execute!(stdout, crossterm::event::EnableMouseCapture).unwrap();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut depths = vec![0]; // to restore selections when moving back
    let mut list: StatefulList = StatefulList::new();
    list.items = tree.get(&cwd);

    loop {
        terminal
            .draw(|frame| {
                list.render(
                    frame,
                    format!(
                        "Files - {:?} {} ({:.0?})",
                        cwd.file_name().unwrap(),
                        list.items.len(),
                        elapsed
                    ),
                );
            })
            .expect("failed to draw frame");

        let mut interact = || {
            if let Some(selected) = list.state.selected() {
                let i = &list.items[selected];
                if i.is_dir {
                    cwd = i.path.clone();
                    depths.push(selected);
                    list.items = tree.get(&cwd);
                } else {
                    Command::new("xdg-open")
                        .arg(i.path.clone())
                        .spawn()
                        .unwrap();
                }
            }
        };
        match event::read().unwrap() {
            Event::Key(key) => match key.code {
                KeyCode::Char('k') => list.state.select_previous(),
                KeyCode::Char('j') => list.state.select_next(),
                KeyCode::Char('G') => list.state.select_last(),
                KeyCode::Char('g') => list.state.select_first(),
                KeyCode::Char('-') => {
                    if depths.len() >= 2 {
                        cwd.pop();
                        list.items = tree.get(&cwd);
                        list.state.select(Some(depths.pop().unwrap()));
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Enter => {
                    interact();
                }
                _ => {}
            },
            Event::Mouse(MouseEvent { kind, row, .. }) => match kind {
                MouseEventKind::Down(_) => {
                    if row >= list.area.y && row < list.area.y + list.area.height {
                        let index = (row - list.area.y - 1) as usize;
                        if let Some(selected) = list.state.selected() {
                            if selected == index {
                                interact();
                            } else {
                                list.state.select(Some(index));
                            }
                        } else {
                            list.state.select(Some(index));
                        }
                    }
                }
                MouseEventKind::ScrollDown => {
                    list.state.select_next();
                }
                MouseEventKind::ScrollUp => {
                    list.state.select_previous();
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
