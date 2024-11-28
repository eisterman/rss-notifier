CREATE TABLE smtp_settings (
    id INTEGER PRIMARY KEY,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    from_email TEXT NOT NULL,
    from_name TEXT NOT NULL,
    to_email TEXT NOT NULL,
    auth_user TEXT NOT NULL,
    auth_password TEXT NOT NULL
);