//! An interactive prompt to select a commit.

use lib::core::node_descriptors::NodeDescriptor;
use lib::git::{Commit, NonZeroOid};

/// Prompt the user to select a commit from the provided list
/// of commits, and returns the OID of the selected commit.
#[cfg(unix)]
pub fn prompt_select_commit(
    header: Option<&str>,
    initial_query: &str,
    commits: Vec<Commit>,
    commit_descriptors: &mut [&mut dyn NodeDescriptor],
) -> eyre::Result<Option<NonZeroOid>> {
    skim::prompt_skim(header, initial_query, commits, commit_descriptors)
}

#[cfg(not(unix))]
pub fn prompt_select_commit(
    header: Option<&str>,
    initial_query: &str,
    commits: Vec<Commit>,
    commit_descriptors: &mut [&mut dyn NodeDescriptor],
) -> eyre::Result<Option<NonZeroOid>> {
    unimplemented!("Non-unix targets are currently unsupported for prompting")
}

#[cfg(unix)]
mod skim {
    use eyre::eyre;
    use std::borrow::Cow;
    use std::sync::Arc;

    use itertools::Itertools;

    use lib::core::formatting::Glyphs;
    use lib::core::node_descriptors::{render_node_descriptors, NodeDescriptor, NodeObject};
    use lib::git::{Commit, NonZeroOid};

    use skim::{
        prelude::SkimOptionsBuilder, AnsiString, DisplayContext, ItemPreview, Matches,
        PreviewContext, Skim, SkimItem, SkimItemReceiver, SkimItemSender,
    };

    #[derive(Debug)]
    pub struct CommitSkimItem {
        pub oid: NonZeroOid,
        pub styled_summary: String,
        pub styled_preview: String,
    }

    impl SkimItem for CommitSkimItem {
        fn text(&self) -> Cow<'_, str> {
            AnsiString::parse(&self.styled_summary).into_inner()
        }

        fn display<'b>(&'b self, context: DisplayContext<'b>) -> AnsiString<'b> {
            let mut text = AnsiString::parse(&self.styled_summary);
            match context.matches {
                Matches::CharIndices(indices) => {
                    text.override_attrs(
                        indices
                            .iter()
                            .map(|&i| {
                                (
                                    context.highlight_attr,
                                    (u32::try_from(i).unwrap(), u32::try_from(i + 1).unwrap()),
                                )
                            })
                            .collect(),
                    );
                }
                Matches::CharRange(start, end) => {
                    text.override_attrs(vec![(
                        context.highlight_attr,
                        (u32::try_from(start).unwrap(), u32::try_from(end).unwrap()),
                    )]);
                }
                Matches::ByteRange(start, end) => {
                    let start = text.stripped()[..start].chars().count();
                    let end = start + text.stripped()[start..end].chars().count();
                    text.override_attrs(vec![(
                        context.highlight_attr,
                        (u32::try_from(start).unwrap(), u32::try_from(end).unwrap()),
                    )]);
                }
                Matches::None => (),
            }
            text
        }

        fn preview(&self, _context: PreviewContext) -> ItemPreview {
            ItemPreview::AnsiText(self.styled_preview.to_owned())
        }
    }

    impl CommitSkimItem {
        fn from_descriptors(
            commit: &Commit,
            commit_descriptors: &mut [&mut dyn NodeDescriptor],
        ) -> eyre::Result<Self> {
            let glyphs = Glyphs::pretty();
            let styled_summary = render_node_descriptors(
                &glyphs,
                &NodeObject::Commit {
                    commit: commit.clone(),
                },
                commit_descriptors,
            )?;

            Ok(CommitSkimItem {
                oid: commit.get_oid(),
                styled_summary: glyphs.render(styled_summary)?,
                styled_preview: Glyphs::pretty().render(commit.friendly_preview()?)?,
            })
        }
    }

    #[cfg(unix)]
    pub fn prompt_skim(
        header: Option<&str>,
        initial_query: &str,
        commits: Vec<Commit>,
        commit_descriptors: &mut [&mut dyn NodeDescriptor],
    ) -> eyre::Result<Option<NonZeroOid>> {
        let options = SkimOptionsBuilder::default()
            .height(Some("100%"))
            .preview(Some(""))
            .preview_window(Some("up:70%"))
            .sync(true) // Consume all items before displaying selector.
            .bind(vec!["Enter:accept"])
            .header(header)
            .query(Some(initial_query))
            .build()
            .map_err(|e| eyre!("building Skim options failed: {}", e))?;

        let items: Vec<CommitSkimItem> = commits
            .iter()
            .map(|commit| CommitSkimItem::from_descriptors(commit, commit_descriptors))
            .try_collect()?;

        let rx_item = {
            let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = skim::prelude::unbounded();
            for i in items {
                tx_item.send(Arc::new(i))?;
            }
            rx_item
        };

        match Skim::run_with(&options, Some(rx_item)) {
            Some(result) => {
                if result.is_abort {
                    return Ok(None);
                }
                let selected = result
                    .selected_items
                    .first()
                    .and_then(|item| (*item).as_any().downcast_ref::<CommitSkimItem>());
                Ok(selected.map(|c| c.oid))
            }
            None => Ok(None),
        }
    }
}
