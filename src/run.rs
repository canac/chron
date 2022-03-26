use crate::schema::run;

#[derive(Insertable, Queryable)]
#[table_name = "run"]
pub struct Run {
    pub id: i32,
    pub name: String,
    pub timestamp: chrono::NaiveDateTime,
    pub status_code: Option<i32>,
}
