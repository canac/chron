use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;

/// Read the status line from an HTTP response
pub async fn read_status(reader: &mut BufReader<TcpStream>) -> Result<String> {
    let mut line = String::new();

    // Parse the status line looking for 200
    reader.read_line(&mut line).await?;
    let Some(status) = line.split_whitespace().nth(1) else {
        bail!("Malformed status line")
    };
    Ok(status.to_owned())
}

/// Read the headers from an HTTP response and ensure that they are from a chron server
pub async fn validate_headers(reader: &mut BufReader<TcpStream>) -> Result<()> {
    let mut line = String::new();

    // Parse the headers looking for X-Powered-By: chron
    let mut valid_server = false;
    loop {
        line.clear();
        reader.read_line(&mut line).await?;
        if line.trim().is_empty() {
            break;
        }

        if let Some((key, value)) = line.trim_end().split_once(':')
            && key.trim().eq_ignore_ascii_case("x-powered-by")
            && value.trim() == "chron"
        {
            valid_server = true;
        }
    }

    if !valid_server {
        bail!("Server is not a chron server");
    }
    Ok(())
}
