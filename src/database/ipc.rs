use anyhow::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wincode::{SchemaRead, SchemaWrite, config::DefaultConfig};

#[derive(SchemaRead, SchemaWrite)]
pub enum Request {
    Connect,
    Trigger { name: String },
    Terminate { name: String },
}

#[derive(SchemaRead, SchemaWrite)]
pub enum TriggerResult {
    Started,
    Running { pid: u32 },
    NotFound,
}

#[derive(SchemaRead, SchemaWrite)]
pub enum Response {
    Connect,
    Trigger { result: TriggerResult },
    Terminate { pid: Option<u32> },
}

/// Serialize a value and write it as a length-prefixed message
pub async fn send<T: SchemaWrite<DefaultConfig, Src = T> + ?Sized + Sync, W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &T,
) -> Result<()> {
    let bytes = wincode::serialize(value)?;
    writer.write_all(&bytes.len().to_le_bytes()).await?;
    writer.write_all(&bytes).await?;
    Ok(())
}

/// Read a length-prefixed message and deserialize it
pub async fn receive<T: for<'a> SchemaRead<'a, DefaultConfig, Dst = T>, R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<T> {
    let mut len_bytes = [0u8; size_of::<usize>()];
    reader.read_exact(&mut len_bytes).await?;
    let length = usize::from_le_bytes(len_bytes);
    let mut buf = vec![0u8; length];
    reader.read_exact(&mut buf).await?;
    Ok(wincode::deserialize(&buf)?)
}
