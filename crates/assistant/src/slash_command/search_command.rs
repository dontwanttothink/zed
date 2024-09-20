use super::{
    create_label_for_command,
    file_command::{build_entry_output_section, codeblock_fence_for_path},
    SlashCommand, SlashCommandOutput,
};
use anyhow::Result;
use assistant_slash_command::{ArgumentCompletion, SlashCommandOutputSection};
use feature_flags::FeatureFlag;
use gpui::{AppContext, Task, WeakView};
use language::{CodeLabel, LineEnding, LspAdapterDelegate};
use semantic_index::{LoadedSearchResult, SemanticDb};
use std::{
    fmt::Write,
    sync::{atomic::AtomicBool, Arc},
};
use ui::{prelude::*, IconName};
use workspace::Workspace;

pub(crate) struct SearchSlashCommandFeatureFlag;

impl FeatureFlag for SearchSlashCommandFeatureFlag {
    const NAME: &'static str = "search-slash-command";
}

pub(crate) struct SearchSlashCommand;

impl SlashCommand for SearchSlashCommand {
    fn name(&self) -> String {
        "search".into()
    }

    fn label(&self, cx: &AppContext) -> CodeLabel {
        create_label_for_command("search", &["--n"], cx)
    }

    fn description(&self) -> String {
        "semantic search".into()
    }

    fn menu_text(&self) -> String {
        "Semantic Search".into()
    }

    fn requires_argument(&self) -> bool {
        true
    }

    fn complete_argument(
        self: Arc<Self>,
        _arguments: &[String],
        _cancel: Arc<AtomicBool>,
        _workspace: Option<WeakView<Workspace>>,
        _cx: &mut WindowContext,
    ) -> Task<Result<Vec<ArgumentCompletion>>> {
        Task::ready(Ok(Vec::new()))
    }

    fn run(
        self: Arc<Self>,
        arguments: &[String],
        _context_slash_command_output_sections: &[SlashCommandOutputSection<language::Anchor>],
        _context_buffer: language::BufferSnapshot,
        workspace: WeakView<Workspace>,
        _delegate: Option<Arc<dyn LspAdapterDelegate>>,
        cx: &mut WindowContext,
    ) -> Task<Result<SlashCommandOutput>> {
        let Some(workspace) = workspace.upgrade() else {
            return Task::ready(Err(anyhow::anyhow!("workspace was dropped")));
        };
        if arguments.is_empty() {
            return Task::ready(Err(anyhow::anyhow!("missing search query")));
        };

        let mut limit = None;
        let mut query = String::new();
        for part in arguments {
            if let Some(parameter) = part.strip_prefix("--") {
                if let Ok(count) = parameter.parse::<usize>() {
                    limit = Some(count);
                    continue;
                }
            }

            query.push_str(part);
            query.push(' ');
        }
        query.pop();

        if query.is_empty() {
            return Task::ready(Err(anyhow::anyhow!("missing search query")));
        }

        let project = workspace.read(cx).project().clone();
        let fs = project.read(cx).fs().clone();
        let Some(project_index) =
            cx.update_global(|index: &mut SemanticDb, cx| index.project_index(project, cx))
        else {
            return Task::ready(Err(anyhow::anyhow!("no project indexer")));
        };

        cx.spawn(|cx| async move {
            let results = project_index
                .read_with(&cx, |project_index, cx| {
                    project_index.search(query.clone(), limit.unwrap_or(5), cx)
                })?
                .await?;

            let loaded_results = SemanticDb::load_results(results, &fs, &cx).await?;

            let output = cx
                .background_executor()
                .spawn(async move {
                    let mut text = format!("Search results for {query}:\n");
                    let mut sections = Vec::new();
                    for LoadedSearchResult {
                        path,
                        range,
                        full_path,
                        file_content,
                        row_range,
                    } in loaded_results
                    {
                        let section_start_ix = text.len();
                        text.push_str(&codeblock_fence_for_path(
                            Some(&path),
                            Some(row_range.clone()),
                        ));

                        let mut excerpt = file_content[range].to_string();
                        LineEnding::normalize(&mut excerpt);
                        text.push_str(&excerpt);
                        writeln!(text, "\n```\n").unwrap();
                        let section_end_ix = text.len() - 1;
                        sections.push(build_entry_output_section(
                            section_start_ix..section_end_ix,
                            Some(&full_path),
                            false,
                            Some(row_range.start() + 1..row_range.end() + 1),
                        ));
                    }

                    let query = SharedString::from(query);
                    sections.push(SlashCommandOutputSection {
                        range: 0..text.len(),
                        icon: IconName::MagnifyingGlass,
                        label: query,
                        metadata: None,
                    });

                    SlashCommandOutput {
                        text,
                        sections,
                        run_commands_in_text: false,
                    }
                })
                .await;

            Ok(output)
        })
    }
}
