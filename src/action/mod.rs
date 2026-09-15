/*! An executable, revertable step of an install

The [`Action`] trait, its state machine and the base actions every product needs come from
the [`installer`] framework; what this module adds is the composite actions which are
specific to an FNO host ([`InstallCardanoNode`](cardano::InstallCardanoNode),
[`GenerateValidatorKey`](midnight::GenerateValidatorKey), and the rest).

During the plan phase a [`Planner`](crate::planner::Planner) calls an [`Action`]'s `plan`
function, which may accept any arguments and returns a [`StatefulAction`]. `plan` is also
where an action decides it is already done, marking itself
[`Completed`](ActionState::Completed) or [`Skipped`](ActionState::Skipped) so that a second
run is a no-op — the same property the shell steps got from their `[[ -s $file ]]` guards.

Later [`InstallPlan`](crate::InstallPlan) calls [`try_execute`](StatefulAction::try_execute)
on each [`StatefulAction`], and [`try_revert`](StatefulAction::try_revert) in reverse order
if something fails.

How fine-grained an [`Action`] should be is decided by the unit of reversion: anything that
can fail halfway and needs undoing on its own belongs in its own action.
*/

pub mod cardano;
pub mod dbsync;
pub mod midnight;
pub mod postgres;
pub mod wireguard;

pub use installer::action::{base, Action, ActionDescription, ActionState, StatefulAction};

pub(crate) use installer::action::fold_errors;

/** Wrap an action in the state its `plan` decided on

The framework's constructors are one per state; an action whose `plan` works the state out
(is the key already there? is the package already installed?) wants the state itself.
[`Progress`](ActionState::Progress) is not a state planning ever produces — it is what a
half-finished execution leaves behind — so it is treated as work still to do.
*/
pub(crate) fn planned<A: Action>(action: A, state: ActionState) -> StatefulAction<A> {
    match state {
        ActionState::Completed => StatefulAction::completed(action),
        ActionState::Skipped => StatefulAction::skipped(action),
        ActionState::Uncompleted | ActionState::Progress => StatefulAction::uncompleted(action),
    }
}
