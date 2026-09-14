use std::io::IsTerminal;

use tracing_subscriber::{
    filter::Directive, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

#[derive(Clone, Default, Debug, clap::ValueEnum)]
pub enum Logger {
    #[default]
    Compact,
    Full,
    Pretty,
    Json,
}

impl std::fmt::Display for Logger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let logger = match self {
            Self::Compact => "compact",
            Self::Full => "full",
            Self::Pretty => "pretty",
            Self::Json => "json",
        };
        write!(f, "{logger}")
    }
}

#[derive(clap::Args, Debug, Default)]
pub struct Instrumentation {
    /// Enable debug logs, `-vv` for trace
    #[clap(short = 'v', long, env = "MIDNIGHT_INSTALLER_VERBOSITY", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Which logger to use
    #[clap(long, env = "MIDNIGHT_INSTALLER_LOGGER", default_value_t = Default::default(), global = true)]
    pub logger: Logger,

    /// Tracing directives, comma delimited
    ///
    /// See <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives>
    #[clap(long = "log-directive", env = "MIDNIGHT_INSTALLER_LOG_DIRECTIVES", value_delimiter = ',', num_args = 0.., global = true)]
    pub log_directives: Vec<Directive>,
}

impl Instrumentation {
    pub fn log_level(&self) -> &'static str {
        match self.verbose {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    }

    pub fn setup(&self) -> anyhow::Result<()> {
        let registry = tracing_subscriber::registry().with(self.filter_layer()?);

        // Everything goes to stderr, so stdout stays the report a caller may pipe
        let ansi = std::io::stderr().is_terminal();
        match self.logger {
            Logger::Compact => registry
                .with(
                    tracing_subscriber::fmt::Layer::new()
                        .with_ansi(ansi)
                        .with_writer(std::io::stderr)
                        .compact(),
                )
                .try_init()?,
            Logger::Full => registry
                .with(
                    tracing_subscriber::fmt::Layer::new()
                        .with_ansi(ansi)
                        .with_writer(std::io::stderr),
                )
                .try_init()?,
            Logger::Pretty => registry
                .with(
                    tracing_subscriber::fmt::Layer::new()
                        .with_ansi(ansi)
                        .with_writer(std::io::stderr)
                        .pretty(),
                )
                .try_init()?,
            Logger::Json => registry
                .with(
                    tracing_subscriber::fmt::Layer::new()
                        .with_ansi(ansi)
                        .with_writer(std::io::stderr)
                        .json(),
                )
                .try_init()?,
        }

        Ok(())
    }

    fn filter_layer(&self) -> anyhow::Result<EnvFilter> {
        let mut filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(self.log_level()));

        for directive in &self.log_directives {
            filter = filter.add_directive(directive.clone());
        }

        Ok(filter)
    }
}
