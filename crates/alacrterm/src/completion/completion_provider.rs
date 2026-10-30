//! Shared query contract for immediate and filesystem-backed providers.
use super::{
    SuggestionSettings,
    completion::{Candidate, Kind, Source},
    path_suggestions::PathQuery,
    project_completions,
    scoring::match_score,
};
pub(super) struct Context<'a> {
    pub text: &'a str,
    pub cursor: usize,
    pub powershell: bool,
    pub history: &'a [String],
    pub commands: &'a [String],
    pub settings: &'a SuggestionSettings,
    pub path: Option<PathQuery>,
}
impl Context<'_> {
    fn has_command(&self, name: &str) -> bool {
        self.commands
            .binary_search_by(|command| command.as_str().cmp(name))
            .is_ok()
    }
}
/// Prefix-only history matching, with unique recency scores across the cache.
pub(super) fn history_score(
    query: &str,
    command: &str,
    powershell: bool,
    index: usize,
) -> Option<i64> {
    let folded;
    let (query, command) = if powershell {
        folded = (query.to_lowercase(), command.to_lowercase());
        (folded.0.as_str(), folded.1.as_str())
    } else {
        (query, command)
    };
    command.starts_with(query).then(|| {
        if command == query {
            1500
        } else {
            1100 - index.min(999) as i64
        }
    })
}
#[derive(Clone, Copy)]
pub(super) enum Provider {
    History,
    Commands,
    Specifications,
    Paths,
    Project,
}
pub(super) const IMMEDIATE: [Provider; 3] = [
    Provider::History,
    Provider::Commands,
    Provider::Specifications,
];
pub(super) enum Work {
    Ready(Vec<Candidate>),
    Lookup(Lookup),
}
#[derive(Clone)]
pub(super) enum Lookup {
    Paths(PathQuery),
    Project(project_completions::Query),
}
impl Provider {
    pub fn query(self, context: &Context<'_>) -> Option<Work> {
        if !context.settings.enabled {
            return None;
        }
        let at_end = context.cursor == context.text.len();
        match self {
            Self::History if context.settings.history && at_end => Some(Work::Ready(
                context
                    .history
                    .iter()
                    .enumerate()
                    .filter_map(|(index, command)| {
                        let score =
                            history_score(context.text, command, context.powershell, index)?;
                        Some(Candidate::new(
                            command.clone(),
                            context.text.len(),
                            Kind::History,
                            Source::History,
                            score,
                        ))
                    })
                    .collect(),
            )),
            Self::Commands if context.settings.commands => {
                let end = context
                    .text
                    .find(char::is_whitespace)
                    .unwrap_or(context.text.len());
                if context.cursor > end
                    || !context.text[..end]
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || "_.:+-".contains(ch))
                {
                    return None;
                }
                let prefix = &context.text[..context.cursor];
                Some(Work::Ready(
                    context
                        .commands
                        .iter()
                        .filter(|command| !command.starts_with('_'))
                        .filter_map(|command| {
                            let score = match_score(
                                prefix,
                                command,
                                context.powershell,
                                context.settings.fuzzy,
                            )?;
                            let mut item = Candidate::new(
                                command.clone(),
                                0,
                                Kind::Command,
                                Source::Shell,
                                score,
                            );
                            item.replacement = 0..end;
                            Some(item)
                        })
                        .collect(),
                ))
            }
            Self::Specifications if context.settings.specifications && at_end => {
                if !context.has_command(context.text.split_whitespace().next().unwrap_or("")) {
                    return None;
                }
                Some(Work::Ready(
                    super::command_specs::candidates(context.text)
                        .into_iter()
                        .map(|(text, description)| {
                            let kind = if text
                                .split_whitespace()
                                .last()
                                .is_some_and(|word| word.starts_with('-'))
                            {
                                Kind::Option
                            } else {
                                Kind::Subcommand
                            };
                            let score = if text == context.text { 1500 } else { 1250 };
                            let mut item = Candidate::new(
                                text,
                                context.text.len(),
                                kind,
                                Source::Specification,
                                score,
                            );
                            item.description = Some(description.into());
                            item
                        })
                        .collect(),
                ))
            }
            Self::Paths if context.settings.paths => {
                Some(Work::Lookup(Lookup::Paths(context.path.clone()?)))
            }
            Self::Project if context.settings.dynamic => {
                let query = project_completions::Query::parse(context.path.as_ref()?)?;
                context
                    .has_command(query.command())
                    .then_some(Work::Lookup(Lookup::Project(query)))
            }
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_candidates_exclude_underscore_prefixed_shell_helpers() {
        let settings = SuggestionSettings::default();
        let commands = ["__git", "_git", "git", "git_helper"].map(String::from);
        for powershell in [false, true] {
            for text in ["", "g", "_", "__"] {
                let context = Context {
                    text,
                    cursor: text.len(),
                    powershell,
                    history: &[],
                    commands: &commands,
                    settings: &settings,
                    path: None,
                };
                let Some(Work::Ready(items)) = Provider::Commands.query(&context) else {
                    panic!("expected command candidates");
                };
                assert!(items.iter().all(|item| !item.insert_text.starts_with('_')));
                if text == "g" {
                    assert!(items.iter().any(|item| item.insert_text == "git"));
                    assert!(items.iter().any(|item| item.insert_text == "git_helper"));
                }
            }
        }
    }

    #[test]
    fn the_same_context_selects_immediate_or_deferred_providers_by_policy() {
        let settings = SuggestionSettings::default();
        let commands = vec!["git".to_string()];
        let path = PathQuery::parse("git switch ma", 13, false).unwrap();
        let context = |settings| Context {
            text: "git switch ma",
            cursor: 13,
            powershell: false,
            history: &[],
            commands: &commands,
            settings,
            path: Some(path.clone()),
        };
        assert!(matches!(
            Provider::Project.query(&context(&settings)),
            Some(Work::Lookup(Lookup::Project(_)))
        ));
        let settings = SuggestionSettings {
            dynamic: false,
            ..settings
        };
        assert!(Provider::Project.query(&context(&settings)).is_none());
        assert!(matches!(
            Provider::Paths.query(&context(&settings)),
            Some(Work::Lookup(Lookup::Paths(_)))
        ));
        let settings = SuggestionSettings {
            enabled: false,
            ..settings
        };
        assert!(Provider::Paths.query(&context(&settings)).is_none());
        assert!(Provider::Commands.query(&context(&settings)).is_none());
    }
}
