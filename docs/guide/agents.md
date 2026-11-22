# Agents

The **Agents** page on your PC runs Claude Code and Codex in folders you choose, and shows each thread as a conversation: what the agent said, the commands it ran, the files it changed, and anything it is waiting on you for. The agents always run on your PC, as you, with your own sign-in. A paired phone can follow a thread, answer its requests and start new ones, but nothing runs on the phone.

## What You Need

| Requirement | Details |
| :--- | :--- |
| **On the PC** | Claude Code (`claude`) or Codex (`codex`) installed and signed in. **Agents › Setup** shows each one's version and how it is signed in. |
| **Folders** | At least one folder added in **Agents › Setup › Folders agents may work in**. An agent works only in a folder you added. |
| **Per-Device Toggle** | **Agents** for each phone, off by default (**Agents › Setup**, under the phone). The phone asks the first time it wants to use them. |
| **Source control** | `git` for changes and commits; the GitHub CLI (`gh`) signed in for pull requests. |

## Threads and Requests

Start a thread with **New thread** (`Ctrl+N`): pick the agent, the folder and the mode, and type what you want done. The thread keeps going while you look at other things. **Needs you** at the top of the list holds the threads waiting for you.

An agent asks before it changes a file or runs a command, unless you chose a mode that lets it. The request shows above the reply box with what it wants to do: **Allow**, **Deny**, or type a few words to deny and tell it why. Your phone gets the same request, and whichever device answers first wins. What G7 keeps and what a phone can see is described in [Privacy and Security](privacy-and-security.md).

## Project Scripts

A folder can have up to 12 scripts: commands you run often, such as `npm run dev` or `cargo test`.

- **Add them** in **Agents › Setup**, on the folder's card, under **Scripts**: a name and a command on one line. G7 never reads a script from the folder itself, so a cloned repository can't add one.
- **Run one** from a thread: the play button in the thread's header lists the folder's scripts. The output streams into the panel beside the thread (`Ctrl+Shift+R`). **Stop** ends it, and **Add to message** puts its last lines into your reply as a quote, without sending anything.
- **How it runs**: like typing the command in a terminal in that folder, as you, with your shell and the same `PATH` the agents get. A tool your shell only sets up for interactive terminals (nvm and the like) may not be found; give its full path, or write `bash -ic '…'` in the command. A worktree thread runs it in its worktree.
- **What it isn't**: a run is not part of the agent's turn. It doesn't use your plan, the agent doesn't see its output unless you add it to a message, and it keeps going after the turn ends. It stops when you press Stop, delete the thread, remove the folder or stop the agent service.
- **From a phone**: the phone always sees the runs and their last lines. It can start and stop scripts only when **Phones can run these** is on for that folder (off by default). The phone shows the command and asks each time, and your PC shows a notice.

If a server starts, the thread shows **A server is listening on port …** with **Open in browser** and **Open on** your phone, as it does for servers the agent starts.

## Pull Requests

The pull request card in the source-control panel (`Ctrl+Shift+G`) and every row of the **Pull Requests** list open the pull request: its description, the changed files and their diff, its checks, and its comments, read through your own `gh`.

- **G7 posts nothing.** There is no way to comment, approve, merge or edit from this view. Use the **Open on GitHub** link for that.
- **Comments on lines of code aren't listed.** They are on GitHub; the view links there.
- **Ask Claude (or Codex) to review it** sends one message in the thread: the pull request's number and link, and an instruction to read it with `gh`, report what is wrong or risky, and change and post nothing. It uses a little of your plan, like any message, and the thread's mode still applies: a supervised thread asks before the agent runs `gh`. With no thread open on that repository, **Ask in a new thread…** starts one.

## Keyboard Shortcuts

These work on the Agents page, while no menu or dialog is open. They are fixed.

| Action | Keys |
| :--- | :--- |
| New thread | `Ctrl+N` |
| Find in threads | `Ctrl+F` |
| Allow the shown request | `Ctrl+Enter`, with the reply box empty |
| Deny the shown request | `Ctrl+Shift+X`, with the reply box empty |
| Stop the agent | `Ctrl+.` |
| Next / previous thread | `Alt+Down` / `Alt+Up` |
| Model and effort | `Ctrl+M` |
| Mode | `Ctrl+Shift+M` |
| Show the last turn's changes (again to close) | `Ctrl+Shift+D` |
| Source control panel | `Ctrl+Shift+G` |
| Run panel | `Ctrl+Shift+R` |
| Thread list · Details | `Ctrl+[` · `Ctrl+]` |
| Next / previous file in a diff | `Ctrl+J` / `Ctrl+K` |
| Answer a question's option | `1` to `9`, outside a text field |
| Rename | `F2` |
| Back or close | `Esc` |
| Add a folder (Setup) | `Ctrl+O` |
| Continue a terminal session here | `Ctrl+Enter` |

With words in the reply box, `Enter` denies the shown request and sends your words to the agent as the reason.

## Settings

In **Agents › Setup**: each folder's MCP servers, restore points, **Keep in step with the remote**, scripts and **Phones can run these**; **Keep this PC awake while an agent works**; **Tell me when a limit resets**; and each phone's switches.

## Limits

- Runs: one run of a script per folder or worktree at a time, four running in a thread and eight on the PC. A run keeps its newest 2,000 lines; finished runs are kept for 30 minutes and are not saved.
- If the agent service crashes (rather than stopping), a script's processes may keep running. They are yours, as in a terminal.
- A server inside a container isn't seen.
