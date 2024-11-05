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

The job name must only contain letters and numbers in kebab case, like "my-1st-job". The `command` value is the command that `chron` will execute when it is running the job. The `workingDir` value optional and sets the current working directory when executing the job. "~" will be expanded to the current user's home directory. The `schedule` value is a cron expression that defines when the job should run. [crontab.guru](https://crontab.guru) is a helpful tool for creating and debugging expressions. Note that `chron` supports an additional sixth field for the year that crontab.guru does not.

**Important note**: `chron` currently uses the [`cron` crate](https://github.com/zslayton/cron) for parsing schedules, and it uses Quartz-style expressions with 0 representing January and 1 representing Sunday, not 1 representing January and 0 representing Sunday like in Unix cron. Consider referring to days and weeks by their name instead of by their index to improve readability and avoid confusion.

## Running chron

Now that you have a chronfile, you can run `chron` like this: `chron chronfile.toml`. Assuming that you provided a valid chronfile, `chron` will run the `webserver` job immediately, run the `online-backup` job every hour, and continue running indefinitely. If you modify the chronfile, `chron` will automatically reload with the new jobs.

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

### `keepAlive`

Startup jobs can set `keepAlive` to configure whether and how the job will be re-run if it terminates. `keepAlive` can be an object with four optional fields, `successes`, `failures`, `limit` and `delay`, or a boolean value. If omitted, `keepAlive` defaults to `{ successes = false, failures = false }`.

#### `keepAlive.successes`

`successes` is a boolean and specifies whether the job should be re-run if it terminates successfully, i.e. exits with a status code of zero. If omitted `successes` defaults to `true`.

#### `keepAlive.failures`

`failures` is a boolean and specifies whether the job should be re-run if it terminates unsuccessfully, i.e. exits with a non-zero status code. If omitted `failures` defaults to `true`.

#### `keepAlive.limit`

`limit` is a positive integer and imposes an upper limit on the number of times that the job will be re-run. For example, if `keepAlive = { limit = 3 }`, the job will be run at most 4 times: once for the initial run plus 3 reruns. `limit` defaults to infinity (i.e. no limit on the number of re-runs) if omitted.

#### `keepAlive.delay`

`delay` is a string and specifies how long to wait after the job terminates before re-running it. For example, if `keepAlive = { delay = "1m 30s" }`, the job will be re-run after 90 seconds. `delay` defaults to `0` (i.e. re-run immediately) if omitted.

#### `keepAlive = false`

`keepAlive` can also be set to the boolean value `false` to easily disable all reruns. `keepAlive = false` is equivalent to `keepAlive = { failures = false, successes = false }`.

#### `keepAlive = true`

`keepAlive` can also be set to the boolean value `true` to easily enable infinite reruns. `keepAlive = true` is equivalent to `keepAlive = { failures = true, successes = true }`.

#### Example

```toml
[startup.webserver]
command = "./start-server.sh"
# Restart the server up to 5 times after a 30 second delay
keepAlive = { limit = 5, delay = "30s" }
```

### `retry`

Scheduled jobs can set `retry` to configure whether and how the job will be re-run if it terminates unsuccessfully, i.e. exits with a non-zero status code. Note that there is no way to configure scheduled jobs to be re-run after terminating successfully because then they would essentially be a startup job. `retry` can be an object with two optional fields, `limit` and `delay`, or a boolean value. If omitted, `retry` defaults to `false`.

#### `retry.limit`

The semantics of `retry.limit` are identical to those of [`keepAlive.limit`](#keepalivelimit).

#### `retry.delay`

The semantics of `retry.delay` are identical to those of [`keepAlive.delay`](#keepalivedelay), except that if omitted, `delay` defaults to one-sixth of the schedule period. For example, if the schedule is `"0 0 * * * *"` to run every hour, the `delay` would default to `10m` or 10 minutes.

#### Example

```toml
[scheduled.online-backup]
command = "./backup.sh"
schedule = "0 0 * * * *"
# Retry the online backup up to 3 times after a 5 minute delay
retry = { limit = 3, delay = "5m" }
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

## HTTP server

`chron` can optionally start a basic HTTP server that lets you see the status of your commands. To enable to HTTP server, provide `chron` with the port port through the `--port` flag or the `PORT` environment variable.

```sh
$ chron chronfile.toml --port=8000

# Or ...

$ PORT=8000 chron chronfile.toml
open http://localhost:8000
```
