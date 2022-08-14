# chron

`chron` is a CLI tool inspired by `chron` that runs commands on startup or on a predetermined schedule. For example, it can be used to launch web servers and startup database processes and restart them if they crash. It can be used to perform daily online backup or clear out temporary files on a monthly basis.

## Basic `chronfile.toml`

Jobs are defined in a `chronfile` written in TOML. Here is a simple example:

```toml
# Define a job named "webserver" that will run when chron starts
[startup.webserver]
command = "./start-server.sh"

# Define a job named "online-backup" that will run on a schedule
[scheduled.online-backup]
command = "./backup.sh"
schedule = "0 0 * * * *" # Run every hour
```

The job name must only contain letters and numbers in kebab case, like "my-1st-job". The `command` value is the command that `chron` will execute when it is running the job. The `schedule` value is a cron expression that defines when the job should run. [crontab.guru](https://crontab.guru) is a helpful tool for creating and debugging expressions. Note that `chron` supports an additional sixth field for the year that crontab.guru does not.

**Important note**: `chron` currently uses the [`cron` crate](https://github.com/zslayton/cron) for parsing schedules, and it uses Quartz-style expressions with 0 representing January and 1 representing Sunday, not 1 representing January and 0 representing Sunday like in Unix cron. Consider referring to days and weeks by their name instead of by their index to improve readability and avoid confusion.

## Running chron

Now that you have a chronfile, you can run chron like this `chron chronfile.toml --port=8000`. Ignore the port argument for now or learn more about it [below](#http-server). Assuming that you provided a valid chronfile, `chron` will run the `webserver` job immediately, run the `online-backup` job every hour, and continue running indefinitely. If you modify the chronfile, `chron` will automatically reload with the new jobs.

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

Startup jobs can set `keepAlive` to configure whether and how the job will be re-run if it terminates. `keepAlive` can be an object with four optional fields, `successes`, `failures`, `limit` and `delay`, or a boolean value. If omitted, `keepAlive` defaults to `{ successes = true, failures = true, delay = "0s" }`.

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

### `makeUpMissedRuns`

Scheduled jobs can set `makeUpMissedRuns` to configure whether to make up for runs that were missed between runs of `chron` and/or delays like the computer being off or asleep. For example, suppose that you have your online backup configured to run every hour. If you shutdown or hibernate your computer at 11:55am and then turn it back on or wake it up at 12:05pm, it will miss the 12pm run unless you have `makeUpMissedRuns` configured.

`makeUpMissedRuns` can be a positive integer or a boolean. If omitted, `makeUpMissedRuns` defaults to `false`.

#### `makeUpMissedRuns = <integer>`

Set `makeUpMissedRuns` to a positive integer to impose an upper limit on the number of times that missed runs will be made up. For example, if `makeUpMissedRuns = 5` and you shutdown your computer at 11:55am on Monday and turn it back on at 12:05pm on Tuesday, your job will have missed 25 runs but only 5 of them will be made up.

#### `makeUpMissedRuns = false`

Set `makeUpMissedRuns` to `false` to disable making up missed runs. This is equivalent to `makeUpMissedRuns = 0`.

#### `makeUpMissedRuns = true`

Set `makeUpMissedRuns` to `true` to enable making up an infinite number of missed runs. Be careful when combining this with frequently-run jobs. For example, if a job is configured to run every 5 seconds and `chron` hasn't run for 3 months, `chron` will run the job over 1.5 million times.

### `config`

At the top level of the chronfile, you can define configuration for the entire `chron` instance.

#### `config.shell`

`shell` is a string that specifies which shell to use when executing all commands in the chronfile. When the `shell` command is run, `chron` passes `-c` as the first argument and the job's command as the second argument. `shell` defaults to the contents of the `$SHELL` environment variable on Unix and `Invoke-Expression` on Windows. If you want to run just one command in a different shell, just put the shell in the job's command itself like this: `command = 'fish -c "my_fish_function"`.

## HTTP server

When run, `chron` starts a basic HTTP server that lets you see the status of your commands and perform basic operations. Set the HTTP server port through the `--port` flag or the `PORT` environment variable.

```sh
$ chron chronfile.toml --port=8000

# Or ...

$ PORT=8000 chron chronfile.toml
```

These are the currently available endpoints:

- `GET /status` lists the defined jobs and whether they are running
- `GET /status/job_name` shows more detailed information about the job `job_name`
- `GET /log/job_name` displays the stdout and stderr logs of the job `job_name`
- `DELETE /log/job_name` clears the lout output of the job `job_name`
- `POST /terminate/job_name` stops the currently executing command of the job `job_name` if it is running
