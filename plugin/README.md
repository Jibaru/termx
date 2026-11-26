# termx Claude Code plugin

This directory is a placeholder for the future termx plugin for
[Claude Code](https://claude.com/claude-code).

## Purpose

termx is built for running many Claude Code sessions side by side. The plugin
will be the bridge between a Claude Code session and the termx window that
hosts it, so that termx can show what each session is doing without scraping
terminal output. The intended scope is:

- reporting session state to termx (working, waiting for input, finished),
- surfacing notifications when a session needs attention,
- letting termx label and group sessions by project and task.

## Status

Nothing is implemented yet. The plugin's structure, its manifest and the
protocol it uses to talk to termx will be defined in a later milestone.
