// Runtime enforcement at the agent boundary: provider adapters, hook installation, pre-action
// authorization, and the hook runtime that verifies actual effects after every tool call.

pub(crate) mod action;
pub(crate) mod adapter;
pub(crate) mod enforce;
pub(crate) mod install;
pub(crate) mod outcome;
pub(crate) mod runtime;
pub(crate) mod shell;
