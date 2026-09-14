use anyhow::Context;

use serde::{Deserialize, Serialize};
use tracing::Instrument;

use super::{Action, ActionDescription};

/// An [`Action`](crate::action::Action) plus the [`ActionState`] which decides whether it
/// still needs to run, or still needs undoing
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
pub struct StatefulAction<A> {
    pub(crate) action: A,
    pub(crate) state: ActionState,
}

/// Where an [`Action`](crate::action::Action) stands relative to the machine
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum ActionState {
    /// Done: skipped on install, reverted on uninstall
    Completed,
    /// Half done: run on both install and uninstall, so a partial composite action finishes
    /// or unwinds
    Progress,
    /// Not done: run on install, skipped on uninstall
    Uncompleted,
    /// Deliberately not our business — skipped both ways
    ///
    /// Used by actions which, while planning, find the machine already in the desired state
    /// by means we did not put there and must not undo.
    Skipped,
}

macro_rules! impl_stateful_common {
    () => {
        /// What this action would do on install, or nothing if it is already done
        pub fn describe_execute(&self) -> Vec<ActionDescription> {
            match self.state {
                ActionState::Completed | ActionState::Skipped => vec![],
                _ => self.action.execute_description(),
            }
        }

        /// What this action would do on uninstall, or nothing if there is nothing to undo
        pub fn describe_revert(&self) -> Vec<ActionDescription> {
            match self.state {
                ActionState::Uncompleted | ActionState::Skipped => vec![],
                _ => self.action.revert_description(),
            }
        }

        /// Execute the action, unless its state says there is nothing to do
        ///
        /// Prefer this over [`Action::execute`], which does not maintain [`ActionState`].
        pub async fn try_execute(&mut self) -> anyhow::Result<()> {
            let span = self.action.tracing_span();
            let synopsis = self.action.tracing_synopsis();
            match self.state {
                ActionState::Completed => {
                    tracing::trace!(parent: &span, "Already done: {synopsis}");
                    Ok(())
                },
                ActionState::Skipped => {
                    tracing::trace!(parent: &span, "Skipped: {synopsis}");
                    Ok(())
                },
                _ => {
                    self.state = ActionState::Progress;
                    tracing::debug!(parent: &span, "Executing: {synopsis}");
                    self.action
                        .execute()
                        .instrument(span.clone())
                        .await
                        .with_context(|| format!("{synopsis} failed"))?;
                    self.state = ActionState::Completed;
                    tracing::debug!(parent: &span, "Completed: {synopsis}");
                    Ok(())
                },
            }
        }

        /// Revert the action, unless its state says there is nothing to undo
        ///
        /// Prefer this over [`Action::revert`], which does not maintain [`ActionState`].
        pub async fn try_revert(&mut self) -> anyhow::Result<()> {
            let span = self.action.tracing_span();
            let synopsis = self.action.tracing_synopsis();
            match self.state {
                ActionState::Uncompleted => {
                    tracing::trace!(parent: &span, "Nothing to revert: {synopsis}");
                    Ok(())
                },
                ActionState::Skipped => {
                    tracing::trace!(parent: &span, "Skipped: {synopsis}");
                    Ok(())
                },
                _ => {
                    self.state = ActionState::Progress;
                    tracing::debug!(parent: &span, "Reverting: {synopsis}");
                    self.action
                        .revert()
                        .instrument(span.clone())
                        .await
                        .with_context(|| format!("Reverting `{synopsis}` failed"))?;
                    self.state = ActionState::Uncompleted;
                    tracing::debug!(parent: &span, "Reverted: {synopsis}");
                    Ok(())
                },
            }
        }
    };
}

impl StatefulAction<Box<dyn Action>> {
    pub fn tracing_synopsis(&self) -> String {
        self.action.tracing_synopsis()
    }

    impl_stateful_common!();
}

impl<A> StatefulAction<A>
where
    A: Action,
{
    pub fn state(&self) -> ActionState {
        self.state
    }

    /// Type erase the action, so a plan can hold a sequence of differing actions
    pub fn boxed(self) -> StatefulAction<Box<dyn Action>>
    where
        Self: 'static,
    {
        StatefulAction {
            action: Box::new(self.action),
            state: self.state,
        }
    }

    pub fn uncompleted(action: A) -> Self {
        Self {
            state: ActionState::Uncompleted,
            action,
        }
    }

    impl_stateful_common!();
}
