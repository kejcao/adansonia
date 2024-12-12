use bytesize::ByteSize;
use clap::Parser;
use crossbeam_deque::{Steal, Worker};
use crossterm::event::{self, Event, KeyCode, MouseEvent, MouseEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::CrosstermBackend;
use ratatui::style::{Color, Modifier};
use ratatui::text::Span;
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    widgets::{Block, List, ListDirection, ListItem, ListState},
};
use ratatui::{Frame, Terminal};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::time::Instant;
use std::{fs, thread};

#[derive(Debug, Clone, Default)]
struct Info {
    path: PathBuf,
    depth: usize,
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
            let depth = self.data[i].depth;
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

        let target = p.components().count() + 1;
        let mut items: Vec<Info> = self.data[start..start + end]
            .iter()
            .filter(|x| x.depth == target)
            .cloned()
            .collect();
        items.sort_by(|a, b| b.size.cmp(&a.size));
        items
    }
}

fn scan(folder: &Path) -> Tree {
    let num_threads = 16;
    let workers: Vec<_> = (0..num_threads)
        .map(|_| Worker::<PathBuf>::new_fifo())
        .collect();
    let stealers: Vec<_> = workers.iter().map(|w| w.stealer()).collect();

    workers[0].push(PathBuf::from(folder));
    thread::scope(|s| {
        let handles: Vec<_> = workers
            .into_iter()
            .enumerate()
            .map(|(i, worker)| {
                let mut stealers = stealers.clone();
                stealers.remove(i);
                stealers.rotate_right(i);

                s.spawn(move || {
                    let mut result = vec![];
                    'main: loop {
                        let p;
                        'outer: {
                            if let Some(i) = worker.pop() {
                                p = i;
                            } else {
                                for s in &stealers {
                                    loop {
                                        match s.steal() {
                                            Steal::Success(i) => {
                                                p = i;
                                                break 'outer;
                                            }
                                            Steal::Empty => break,
                                            Steal::Retry => (),
                                        }
                                    }
                                }
                                break 'main;
                            }
                        };

                        let _ = fs::read_dir(p).map(|it| {
                            for entry in it {
                                let entry = entry.unwrap();
                                let ft = entry.file_type().unwrap();
                                if ft.is_symlink() {
                                    continue;
                                }

                                let metadata = entry.metadata().unwrap();

                                result.push(Info {
                                    path: entry.path().to_path_buf(),
                                    depth: entry.path().components().count(),
                                    size: metadata.size(),
                                    is_dir: metadata.is_dir(),
                                });
                                if ft.is_dir() {
                                    worker.push(entry.path().to_path_buf());
                                }
                            }
                        });
                    }
                    result
                })
            })
            .collect();

        let mut result = vec![Info {
            path: folder.to_path_buf(),
            depth: folder.components().count(),
            size: folder.metadata().unwrap().size(),
            is_dir: true,
        }];
        for handle in handles {
            result.append(&mut handle.join().unwrap());
        }
        result.sort_by(|a, b| a.path.cmp(&b.path));
        return Tree { data: result };
    })
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
            items: vec![],
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
