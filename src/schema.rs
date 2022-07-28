table! {
    checkpoint (id) {
        id -> Integer,
        job -> Text,
        timestamp -> Timestamp,
    }
}

table! {
    run (id) {
        id -> Integer,
        name -> Text,
        timestamp -> Timestamp,
        status_code -> Nullable<Integer>,
    }
}
