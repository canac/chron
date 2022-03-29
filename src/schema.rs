table! {
    run (id) {
        id -> Integer,
        name -> Text,
        timestamp -> Timestamp,
        status_code -> Nullable<Integer>,
    }
}

table! {
    message (id) {
        id -> Integer,
        mailbox -> Text,
        content -> Text,
        timestamp -> Timestamp,
        read -> Bool,
    }
}
