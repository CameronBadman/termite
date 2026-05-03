use std::{env, error::Error};

use crate::{plugins::PluginHost, window_backend};

type InstallPart = Box<dyn FnOnce(&mut Runner)>;

pub(crate) struct Runner {
    shell: String,
    plugins: PluginHost,
}

impl Runner {
    pub(crate) fn new() -> Self {
        Self {
            shell: env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            plugins: PluginHost::new(),
        }
    }

    pub(crate) fn with(mut self, part: impl RunnerPart) -> Self {
        part.install(&mut self);
        self
    }

    pub(crate) fn run(self) -> Result<(), Box<dyn Error>> {
        window_backend::run(self)
    }

    pub(crate) fn into_parts(self) -> (String, PluginHost) {
        (self.shell, self.plugins)
    }

    fn add_plugin(&mut self, plugin: impl crate::plugins::Plugin + 'static) {
        self.plugins.add(plugin);
    }

    #[cfg(test)]
    pub(crate) fn plugin_count(&self) -> usize {
        self.plugins.len()
    }
}

pub(crate) trait RunnerPart {
    fn install(self, runner: &mut Runner);
}

impl<P> RunnerPart for P
where
    P: crate::plugins::Plugin + 'static,
{
    fn install(self, runner: &mut Runner) {
        runner.add_plugin(self);
    }
}

pub(crate) fn parts() -> Parts {
    Parts::new()
}

pub(crate) struct Parts {
    install: Vec<InstallPart>,
}

impl Parts {
    fn new() -> Self {
        Self {
            install: Vec::new(),
        }
    }

    pub(crate) fn with(mut self, part: impl RunnerPart + 'static) -> Self {
        self.install
            .push(Box::new(move |runner| part.install(runner)));
        self
    }
}

impl RunnerPart for Parts {
    fn install(self, runner: &mut Runner) {
        for install in self.install {
            install(runner);
        }
    }
}
