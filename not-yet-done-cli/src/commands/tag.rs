use tusks::tusks;

#[tusks()]
#[command(about = "Manage tags")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Create a new tag (global or project-specific with --project)
    pub fn add(
        #[arg(help = "Tag name")] name: String,
        #[arg(long, help = "Foreground color as hex (e.g. #FFFFFF)")] fg: Option<String>,
        #[arg(long, help = "Background color as hex (e.g. #FF5733)")] bg: Option<String>,
        #[arg(long, help = "Symbol/label prefix (free-form text)")] symbol: Option<String>,
        #[arg(long, help = "Create as project-specific tag (name or ID)")] project: Option<String>,
    ) -> u8 {
        let result: Result<String, not_yet_done_task_core::error::AppError> =
            crate::run_async(|module| async move {
                use not_yet_done_task_core::repository::TagStyle;
                use not_yet_done_task_core::service::TagService;
                use shaku::HasComponent;
                let service: &dyn TagService = module.resolve_ref();
                let style = TagStyle {
                    fg_color: fg,
                    bg_color: bg,
                    symbol,
                };
                if let Some(proj) = project {
                    let tag = service.add_project_tag(name, style, proj).await?;
                    Ok(format!(
                        "✓ Project tag created: [project-tag:{}] {}",
                        tag.id, tag.name
                    ))
                } else {
                    let tag = service.add_global(name, style).await?;
                    Ok(format!(
                        "✓ Global tag created: [global-tag:{}] {}",
                        tag.id, tag.name
                    ))
                }
            });
        match result {
            Ok(msg) => {
                println!("{msg}");
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// List tags
    pub fn list(
        #[arg(long, help = "Show only tags for this project (name or ID)")] project: Option<String>,
        #[arg(long, help = "Show only global tags")] global: bool,
    ) -> u8 {
        let result: Result<Vec<String>, not_yet_done_task_core::error::AppError> =
            crate::run_async(|module| async move {
                use not_yet_done_task_core::service::{TagItem, TagService};
                use shaku::HasComponent;
                let service: &dyn TagService = module.resolve_ref();

                if let Some(proj) = project {
                    let tags = service.list_by_project(proj).await?;
                    Ok(tags
                        .into_iter()
                        .map(|t| {
                            format!(
                                "[project-tag:{}] {}{}",
                                t.id,
                                t.name,
                                fmt_style(
                                    t.fg_color.as_deref(),
                                    t.bg_color.as_deref(),
                                    t.symbol.as_deref()
                                )
                            )
                        })
                        .collect())
                } else if global {
                    let tags = service.list_global().await?;
                    Ok(tags
                        .into_iter()
                        .map(|t| {
                            format!(
                                "[global-tag:{}] {}{}",
                                t.id,
                                t.name,
                                fmt_style(
                                    t.fg_color.as_deref(),
                                    t.bg_color.as_deref(),
                                    t.symbol.as_deref()
                                )
                            )
                        })
                        .collect())
                } else {
                    let items = service.list_all().await?;
                    Ok(items
                        .into_iter()
                        .map(|item| match item {
                            TagItem::Global(t) => format!(
                                "[global-tag:{}] {}{}",
                                t.id,
                                t.name,
                                fmt_style(
                                    t.fg_color.as_deref(),
                                    t.bg_color.as_deref(),
                                    t.symbol.as_deref()
                                )
                            ),
                            TagItem::Project {
                                tag: t,
                                project_name,
                            } => format!(
                                "[project-tag:{}] {} (project: {}){}",
                                t.id,
                                t.name,
                                project_name,
                                fmt_style(
                                    t.fg_color.as_deref(),
                                    t.bg_color.as_deref(),
                                    t.symbol.as_deref()
                                )
                            ),
                        })
                        .collect())
                }
            });
        match result {
            Ok(lines) if lines.is_empty() => {
                println!("No tags found.");
                0
            }
            Ok(lines) => {
                lines.iter().for_each(|l| println!("{l}"));
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Edit a tag
    ///
    /// Use the full tag ID including prefix:
    ///   global-tag:<uuid>   for global tags
    ///   project-tag:<uuid>  for project-specific tags
    ///
    /// Pass an empty string to clear an optional field
    /// (e.g. `--fg ""` removes the foreground color).
    pub fn edit(
        #[arg(help = "Tag ID (global-tag:<uuid> or project-tag:<uuid>)")] id: String,
        #[arg(long, help = "New tag name")] name: Option<String>,
        #[arg(long, help = "New foreground color (empty string clears)")] fg: Option<String>,
        #[arg(long, help = "New background color (empty string clears)")] bg: Option<String>,
        #[arg(long, help = "New symbol (empty string clears)")] symbol: Option<String>,
    ) -> u8 {
        let result: Result<String, not_yet_done_task_core::error::AppError> =
            crate::run_async(|module| async move {
                use not_yet_done_task_core::repository::TagStylePatch;
                use not_yet_done_task_core::service::{TagItem, TagService};
                use shaku::HasComponent;
                let service: &dyn TagService = module.resolve_ref();
                let patch = TagStylePatch {
                    fg_color: fg.map(empty_to_none),
                    bg_color: bg.map(empty_to_none),
                    symbol: symbol.map(empty_to_none),
                };
                let item = service.edit(id, name, patch).await?;
                Ok(match item {
                    TagItem::Global(t) => {
                        format!("✓ Tag updated: [global-tag:{}] {}", t.id, t.name)
                    }
                    TagItem::Project { tag: t, .. } => {
                        format!("✓ Tag updated: [project-tag:{}] {}", t.id, t.name)
                    }
                })
            });
        match result {
            Ok(msg) => {
                println!("{msg}");
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Create a new tag interactively via $EDITOR.
    ///
    /// Opens a temporary YAML file with all tag fields. On save the
    /// file is parsed and the tag is created. If `name:` is empty,
    /// creation is aborted silently. Validation errors (invalid hex
    /// color, unknown project, malformed YAML) re-open the editor with
    /// a `# ERROR:` header so the user can correct the input.
    pub fn new() -> u8 {
        use not_yet_done_task_core::service::{
            annotate_error, new_tag_template, normalize, parse_draft,
        };
        let mut content = new_tag_template();
        loop {
            let edited = match open_editor_inline(&content) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: editor failed: {e}");
                    return 1;
                }
            };
            match parse_draft(&edited) {
                Ok(draft) => {
                    let name = match normalize(draft.name) {
                        Some(n) => n,
                        None => {
                            println!("Aborted (empty name).");
                            return 0;
                        }
                    };
                    let style = TagStyleArgs {
                        fg: normalize(draft.fg_color),
                        bg: normalize(draft.bg_color),
                        symbol: normalize(draft.symbol),
                    };
                    let project = normalize(draft.project);
                    let res = create_tag(name, style, project);
                    return match res {
                        Ok(msg) => {
                            println!("{msg}");
                            0
                        }
                        Err(e) => {
                            content = annotate_error(&edited, &e.to_string());
                            continue;
                        }
                    };
                }
                Err(e) => {
                    content = annotate_error(&edited, &e);
                    continue;
                }
            }
        }
    }

    /// Delete a tag
    ///
    /// Use the full tag ID including prefix:
    ///   global-tag:<uuid>   for global tags
    ///   project-tag:<uuid>  for project-specific tags
    pub fn delete(
        #[arg(help = "Tag ID (global-tag:<uuid> or project-tag:<uuid>)")] id: String,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TagService;
            use shaku::HasComponent;
            let service: &dyn TagService = module.resolve_ref();
            service.delete(id).await
        });
        match result {
            Ok(()) => {
                println!("✓ Tag deleted.");
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    fn empty_to_none(s: String) -> Option<String> {
        if s.is_empty() { None } else { Some(s) }
    }

    fn fmt_style(fg: Option<&str>, bg: Option<&str>, sym: Option<&str>) -> String {
        let parts: Vec<String> = [
            sym.map(|s| format!("sym={s}")),
            fg.map(|c| format!("fg={c}")),
            bg.map(|c| format!("bg={c}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        }
    }

    // ── `tag new` helpers ───────────────────────────────────────────

    struct TagStyleArgs {
        fg: Option<String>,
        bg: Option<String>,
        symbol: Option<String>,
    }

    fn open_editor_inline(initial: &str) -> std::io::Result<String> {
        use std::io::{Read, Write};
        let mut tmp = tempfile::Builder::new()
            .prefix("nyd-tag-")
            .suffix(".yaml")
            .tempfile()?;
        tmp.write_all(initial.as_bytes())?;
        tmp.flush()?;
        let path = tmp.path().to_owned();

        let editor = std::env::var("VISUAL")
            .ok()
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_else(|| "vi".to_string());

        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "{} {}",
                editor,
                shell_escape(&path.display().to_string())
            ))
            .status()?;
        if !status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("editor exited with {status}"),
            ));
        }
        let mut buf = String::new();
        std::fs::File::open(&path)?.read_to_string(&mut buf)?;
        Ok(buf)
    }

    fn shell_escape(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    fn create_tag(
        name: String,
        style: TagStyleArgs,
        project: Option<String>,
    ) -> Result<String, not_yet_done_task_core::error::AppError> {
        crate::run_async(|module| async move {
            use not_yet_done_task_core::repository::TagStyle;
            use not_yet_done_task_core::service::TagService;
            use shaku::HasComponent;
            let service: &dyn TagService = module.resolve_ref();
            let s = TagStyle {
                fg_color: style.fg,
                bg_color: style.bg,
                symbol: style.symbol,
            };
            if let Some(proj) = project {
                let tag = service.add_project_tag(name, s, proj).await?;
                Ok(format!(
                    "✓ Project tag created: [project-tag:{}] {}",
                    tag.id, tag.name
                ))
            } else {
                let tag = service.add_global(name, s).await?;
                Ok(format!(
                    "✓ Global tag created: [global-tag:{}] {}",
                    tag.id, tag.name
                ))
            }
        })
    }
}
