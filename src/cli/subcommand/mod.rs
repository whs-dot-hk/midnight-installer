pub(crate) mod install;
pub(crate) mod plan;
pub(crate) mod status;
pub(crate) mod uninstall;

#[derive(Debug, clap::Subcommand)]
pub enum MidnightInstallerSubcommand {
    /// Describe what a stage would do, without doing it
    Plan(plan::Plan),
    /// Carry out a stage of the FNO build-out
    Install(install::Install),
    /// Undo a stage, following the receipt it wrote
    Uninstall(uninstall::Uninstall),
    /// Report where this host has got to
    Status(status::Status),
}
