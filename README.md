# chron

`chron` is a cross-platform CLI tool inspired by `cron` and `systemd`/`launchd` that runs commands on startup or on a predetermined schedule. For example, it can be used to launch web servers and startup database processes and restart them if they crash. It can be used to perform daily online backup or clear out temporary files on a monthly basis.

## Installation

Install `chron` via Homebrew.

```sh
brew install canac/tap/chron
```

## Basic `chronfile.toml`

Jobs are defined in a `chronfile` written in TOML. Here is a simple example:

```toml
# Define a job named "webserver" that will run when chron starts
[startup.webserver]
command = "./start-server.sh"
workingDir = "~/dev/server"

# Define a job named "online-backup" that will run on a schedule
[scheduled.online-backup]
command = "./backup.sh"
schedule = "0 0 * * * *" # Run every hour
```

The job name must only contain letters and numbers in kebab case, like "my-1st-job". The `command` value is the command that `chron` will execute when it is running the job. The `workingDir` value is optional and sets the current working directory when executing the job. "~" will be expanded to the current user's home directory. The `schedule` value is a cron expression that defines when the job should run. [crontab.guru](https://crontab.guru) is a helpful tool for creating and debugging expressions. Note that `chron` supports an additional sixth field for the year that crontab.guru does not.

**Important note**: `chron` currently uses the [`cron` crate](https://github.com/zslayton/cron) for parsing schedules, and it uses Quartz-style expressions with 0 representing January and 1 representing Sunday, not 1 representing January and 0 representing Sunday like in Unix cron. Consider referring to days and weeks by their name instead of by their index to improve readability and avoid confusion.

## Running chron

Now that you have a chronfile, you can run `chron` like this: `chron run chronfile.toml`. Assuming that you provided a valid chronfile, `chron` will run the `webserver` job immediately, run the `online-backup` job every hour, and continue running indefinitely. If you modify the chronfile, `chron` will automatically reload with the new jobs.

## Configuring chron

There are a few job configuration options available.

### `disabled`

Both startup and schedule jobs can set `disabled` to `true` to be ignored and not run.

```toml
[startup.webserver]
command = "./start-server.sh"
disabled = true
```

Commenting out the job achieves the same effect

```toml
# [startup.webserver]
# command = "./start-server.sh"
```

### `retry`

chron can rerun jobs that fail with a non-zero exit code.

#### `retry.limit`

Set `retry.limit` to control how many times th job will be rerun when it fails. For example, if `retry.limit = 3`, the job will run at most 4 times: once for the initial run plus 3 reruns. Set the limit to `"unlimited"` to rerun it an unlimited number of times. `limit` defaults to 0 (i.e. no retries).

#### `retry.delay`

Set `retry.delay` to control how long to wait after the job terminates before rerunning it. For example, if `retry.delay = "1m 30s"`, the job will be rerun after 90 seconds. If omitted, `delay` defaults to `"0s"` (i.e. rerun immediately) for startup jobs and one-sixth of the schedule period for scheduled jobs. For example, if the schedule is `"0 0 * * * *"` to run every hour, the `delay` would default to `10m` or 10 minutes.

#### Example

```toml
[startup.webserver]
command = "./start-server.sh"
# Restart the server an unlimited number of times with no delay
retry.limit = "unlimited"

[scheduled.online-backup]
command = "./backup.sh"
schedule = "0 0 * * * *"
# Retry the backup up to 5 times after a 30 second delay
retry = { limit = 5, delay = "30s" }
```

### `makeUpMissedRun`

By default, `chron` only executes jobs that were scheduled to run while `chron` itself was running. However, if `chron` is stopped or your computer is powered off, this can cause `chron` to miss scheduled runs. For example, suppose that you have your online backup configured to run every hour. If you shutdown your computer at 11:55am and then turn it back on at 12:05pm, `chron` would miss the 12pm execution.

To change this, scheduled jobs can set `makeUpMissedRun = true`. With this configuration, on startup `chron` will check whether any jobs were scheduled to run since `chron` stopped running and run any that it finds.

`makeUpMissedRun` defaults to `false`.

#### `makeUpMissedRun = true`

Set `makeUpMissedRun` to `true` to enable making up a run that was scheduled to run when `chron` was not running.

#### `makeUpMissedRun = false`

Set `makeUpMissedRun` to `false` to disable making up missed runs. `chron` will only execute jobs that were scheduled to run while `chron` was running.

#### Example

```toml
[scheduled.online-backup]
command = "./backup.sh"
schedule = "0 0 * * * *"
makeUpMissedRun = true
```

### `config`

At the top level of the chronfile, you can define configuration for the entire `chron` instance.

#### `config.shell`

`shell` is a string that specifies which shell to use when executing all commands in the chronfile. When the `shell` command is run, `chron` passes `-c` as the first argument and the job's command as the second argument. `shell` defaults to the contents of the `$SHELL` environment variable on Unix and `Invoke-Expression` on Windows. If you want to run just one command in a different shell, just put the shell in the job's command itself like this: `command = 'fish -c "my_fish_function"`.

#### `config.onError`

`onError` is an optional string that specifies an error handler when any job fails (exits with a non-zero status code). The script will be executed using the configured shell, if any. It is also executed in the job's working directory, if configured.

Information about the current run is passed to the `onError` error handler through environment variables:

- `CHRON_JOB`: The name of the job that failed
- `CHRON_COMMAND`: The command that failed
- `CHRON_EXIT_CODE`: The failed command's exit code

#### Example

```toml
[config]
onError = 'echo "Job $CHRON_JOB ($CHRON_COMMAND) failed with exit code $CHRON_EXIT_CODE" >> /var/log/chron.log'
```

## HTTP server

`chron` also starts a basic HTTP server that lets you see the status of your commands. By default, `chron` will pick an open port to listen on.

```sh
$ chron run chronfile.toml
# ...
Listening on port http://localhost:2748
```

You can also manually specify the port through the `--port` flag or the `PORT` environment variable.

```sh
$ PORT=8000 chron run chronfile.toml
# Or ...
$ chron run chronfile.toml --port=8000

$ open http://localhost:8000
```

## Data Directory

By default `chron` stores its state database and logs in a platform-specific directory. You can manually specify the data directory through the `--data-dir` flag or the `CHRON_DATA_DIR` environment variable. If you want to have two `chron` processes running simultaneously, they will need to have separate data directories to avoid conflicting.

```sh
$ chron run --data-dir=~/.local/share/chron-1 chronfile1.toml &
$ chron run --data-dir=~/.local/share/chron-2 chronfile2.toml
```

## Management CLI

You can view and manage a running `chron` process through the CLI.

### `jobs` Subcommand

`chron jobs` will print a table of the registered jobs, their command, and their status.

```sh
$ chron jobs
+---------------+-------------------+------------------------+
| name          | command           | status                 |
+---------------+-------------------+------------------------+
| online-backup | ./backup.sh       | next run in 40 minutes |
| webserver     | ./start-server.sh | running                |
+---------------+-------------------+------------------------+
```

### `status` Subcommand

`chron status [job]` will print more detailed information about a specific job.

```sh
$ chron status online-backup
command: ./backup.sh
schedule: 0 0 * * * *
status: not running (next run at 2025-01-01 00:00:00 UTC)
```

### `runs` Subcommand

`chron runs [job]` will print a table of the job's most recent runs and their exit codes.

```sh
$ chron runs online-backup
+----------------+----------------+--------+
| time           | execution time | status |
+----------------+----------------+--------+
| 20 minutes ago | 25 seconds     | 0      |
| 1 hour ago     | 27 seconds     | 0      |
| 2 hours ago    | 26 seconds     | 0      |
| 3 hours ago    | 1 second       | 1      |
| 4 hours ago    | 25 seconds     | 0      |
+----------------+----------------+--------+
```

### `logs` Subcommand

`chron logs [job]` will print the stdout and stderr from the job's most recent execution. Pass the `--lines=n` flag to only print the last `n` lines. Pass the `--follow` flag to keep printing logs from a running job as they are written.

```sh
$ chron logs online-backup
[2025-01-01 12:00:00] Starting backup process...
[2025-01-01 12:00:15] Backing up database...
[2025-01-01 12:00:20] Backup completed successfully
```

### `kill` Subcommand

`chron kill [job]` will kill the job's running process, if it is running. The job will run again the next time that it is scheduled to.

```sh
$ chron kill online-backup
Terminated process 12345
```
