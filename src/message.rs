use crate::schema::message;

#[derive(Insertable, Queryable)]
#[table_name = "message"]
pub struct Message {
    pub id: i32,
    pub mailbox: String,
    pub content: String,
    pub timestamp: chrono::NaiveDateTime,
    pub read: bool,
}
