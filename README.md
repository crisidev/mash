# Multiplexed Asynchronous SSH

**Control multiple SSH sessions from a single interactive shell.**

mash connects to multiple remote hosts simultaneously and lets you type commands once to execute them everywhere. It's a Rust reimplementation of [polysh](https://github.com/innogames/polysh) that works with my zsh and starship/powerline prompt.

## Features

- **Parallel SSH sessions** &mdash; connect to dozens or hundreds of hosts at once
- **Interactive multiplexing** &mdash; type a command, see output from all hosts prefixed with their names
- **Host expansion** &mdash; `mash host<1-50>` expands to host1 through host50
- **Shell pattern matching** &mdash; `:enable web*` to target specific hosts with glob patterns
- **Colored output** &mdash; each host gets a distinct color for easy scanning
- **Tab completion** &mdash; completes commands, paths, hostnames, and history
- **Control commands** &mdash; `:list`, `:enable`, `:disable`, `:reconnect`, `:rename`, and more
- **Non-interactive mode** &mdash; pipe commands or use `--command` for scripting
- **Password support** &mdash; `--password-file` for automated password entry
- **Logging** &mdash; optional session logging to file

## Install

### From crates.io

```sh
cargo install mash-ssh
```

### With Nix

```sh
# Run directly
nix run github:crisidev/mash

# Install into profile
nix profile install github:crisidev/mash
```

### From source

```sh
git clone https://github.com/crisidev/mash
cd mash
cargo install --path .
```

## Usage

```sh
# Connect to multiple hosts
mash server1 server2 server3

# Host range expansion
mash web<1-10>
mash db<01-03> cache<1-5>

# Non-interactive: run a command and exit
mash --command "uptime" host<1-20>

# Pipe commands from stdin
echo "hostname && uptime" | mash host<1-5>

# Read hosts from a file
mash --hosts-file servers.txt

# Connect as a specific user
mash --user deploy web<1-10>
```

### Interactive commands

Once connected, type any command to send it to all enabled shells. Prefix with `:` for control commands:

```
mash [● 3] ❯❯❯ uptime          # sent to all hosts
mash [● 3] ❯❯❯ :list           # show shell status
mash [● 3] ❯❯❯ :disable web3   # stop sending to web3
mash [● 3] ❯❯❯ :enable *       # re-enable all
mash [● 3] ❯❯❯ :help           # show all commands
mash [● 3] ❯❯❯ !ls             # run locally
```

### Prompt indicators

| Symbol | Color  | Meaning    |
|--------|--------|------------|
| `●`    | Green  | Idle       |
| `◉`    | Yellow | Running    |
| `◌`    | Blue   | Pending    |
| `✕`    | Red    | Dead       |
| `○`    | Dim    | Disabled   |

### Control commands

| Command                     | Description                                       |
|-----------------------------|---------------------------------------------------|
| `:help`                     | Show help message                                 |
| `:list [PATTERN]`           | List shells and their status                      |
| `:quit`                     | Close all connections and exit                    |
| `:enable [PATTERN]`         | Enable matching shells                            |
| `:disable [PATTERN]`        | Disable matching shells                           |
| `:reconnect [PATTERN]`      | Reconnect dead shells                             |
| `:add HOST...`              | Add new SSH connections                           |
| `:purge [PATTERN]`          | Remove disabled shells                            |
| `:rename NAME`              | Rename enabled shells                             |
| `:send_ctrl LETTER [PATTERN]` | Send a control character (e.g. `:send_ctrl c`)  |
| `:reset_prompt [PATTERN]`   | Re-send prompt initialization                     |
| `:chdir [PATH]`             | Change local working directory                    |
| `:hide_password`            | Disable echo/debug/logging for password entry     |
| `:set_debug y\|n [PATTERN]` | Toggle debug output per shell                     |
| `:export_vars`              | Set MASH_RANK/NAME/NR_SHELLS on each shell      |
| `:set_log [PATH]`           | Set or disable the log file                       |
| `:show_read_buffer [PATTERN]` | Show buffered output from shell startup         |

`PATTERN` supports `*` and `?` wildcards matching against shell display names or last output line.

## Options

```
  --hosts-file       Read hostnames from a file, one per line
  --command          Command to run on remote shells (non-interactive)
  --ssh              SSH command template (default: exec ssh -oLogLevel=Quiet -t %(host)s %(port)s)
  --user             Remote user to log in as
  --no-color         Disable colored output
  --password-file    Read password from file (use - for interactive prompt)
  --log-file         Log session to file
  --abort-errors     Abort if any shell fails to initialize
  --debug            Print debug information
```

## Compared to polysh

mash is a ground-up Rust rewrite of polysh with the same feature set. Key differences:

- **Faster startup** &mdash; compiled binary, no Python interpreter overhead
- **Async I/O** &mdash; tokio-based event loop instead of Python's select()
- **Modern shell compatibility** &mdash; works with zsh + starship/powerlevel10k out of the box

## License

MIT
