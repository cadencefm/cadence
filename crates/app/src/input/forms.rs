//! Keys inside authoring forms.
use super::*;

impl App {
    pub(crate) fn begin_authoring(&mut self, draft: Draft) -> Result<Outcome> {
        let origin = Some(self.navigation());
        self.clear_browser();
        self.modal = ActiveModal::Authoring {
            draft: Box::new(draft),
            origin,
        };
        self.input.count = None;
        self.input.mode = Mode::Authoring;
        Ok(self.release_held())
    }
    pub(crate) fn handle_authoring(&mut self, key: KeyEvent) -> Result<Outcome> {
        let (mut draft, origin) = self
            .modal
            .take_authoring()
            .context("Authoring draft missing")?;
        match draft.handle_key(self.doc.project(), key) {
            DraftOutcome::Continue => {
                self.modal = ActiveModal::Authoring { draft, origin };
                Ok(Outcome::default())
            }
            DraftOutcome::Cancel => {
                self.input.mode = Mode::Normal;
                if let Some(origin) = origin {
                    self.restore_navigation(origin);
                }
                self.status = "Authoring cancelled · musical state unchanged".into();
                Ok(Outcome::default())
            }
            DraftOutcome::Submit(submission) => {
                let target = submission.target;
                let result = if let Some(id) = target {
                    if self.pattern(id).is_none() {
                        Err(anyhow::anyhow!("The edited pattern no longer exists"))
                    } else {
                        self.apply(Action::Edit {
                            edit: Edit::PutPattern {
                                pattern: Pattern {
                                    id,
                                    name: submission.name,
                                    kind: submission.kind,
                                },
                            },
                        })
                    }
                } else {
                    self.apply(Action::CreatePattern {
                        name: submission.name,
                        kind: submission.kind,
                    })
                };
                match result {
                    Ok(outcome) => {
                        self.input.mode = Mode::Normal;
                        if let Some(id) = target {
                            self.select_pattern(id)?;
                        }
                        Ok(outcome)
                    }
                    Err(error) => {
                        draft.set_error(format!("{error:#}"));
                        self.modal = ActiveModal::Authoring { draft, origin };
                        self.input.mode = Mode::Authoring;
                        Ok(Outcome::default())
                    }
                }
            }
        }
    }
}
