CREATE TABLE rss_feeds (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    feed_url TEXT NOT NULL,
    last_pub_date TEXT DEFAULT NULL -- ISO 8601 format e.g. "2023-10-05T14:48:00+02:00"
);
