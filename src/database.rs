use std::{path::PathBuf, sync::Arc};

use jiff::Timestamp;
use oauth2::{AccessToken, RefreshToken};
use redb::{ReadableDatabase as _, TableDefinition};

use crate::oauth::Authentication;

#[derive(Clone)]
pub struct Database {
    connection: Arc<redb::Database>,
}

impl Database {
    pub fn create(database_path: PathBuf) -> Self {
        Self {
            connection: Arc::new({
                let db = redb::Database::create(database_path).unwrap();

                let txn = db.begin_write().unwrap();
                txn.open_table(OAuth::TABLE).unwrap();
                txn.open_table(UpdateDate::TABLE).unwrap();
                txn.commit().unwrap();

                db
            }),
        }
    }

    pub fn oauth(&self) -> OAuth<'_> {
        OAuth { database: self }
    }

    pub fn update_date(&self) -> UpdateDate<'_> {
        UpdateDate { database: self }
    }
}

type OAuthStorage<'s> = (&'s str, Option<&'s str>, i64);
pub struct OAuth<'a> {
    database: &'a Database,
}
impl<'a> OAuth<'a> {
    const TABLE: TableDefinition<'static, (), OAuthStorage<'static>> =
        TableDefinition::new("oauth");

    pub fn set(&self, auth: Authentication) {
        let write_txn = self.database.connection.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(Self::TABLE).unwrap();
            table
                .insert(
                    (),
                    &(
                        auth.access_token.secret().as_str(),
                        auth.refresh_token.as_ref().map(|t| t.secret().as_str()),
                        auth.expires_at.as_millisecond(),
                    ),
                )
                .unwrap();
        }
        write_txn.commit().unwrap();
    }

    pub fn delete(&self) {
        let write_txn = self.database.connection.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(Self::TABLE).unwrap();
            table.remove(()).unwrap();
        }
        write_txn.commit().unwrap();
    }

    pub fn get(&self) -> Option<Authentication> {
        let read_txn = self.database.connection.begin_read().unwrap();
        let table = read_txn.open_table(Self::TABLE).unwrap();

        if let Some(value) = table.get(()).unwrap() {
            let (access_token, refresh_token, expires_at) = value.value();

            Some(Authentication {
                access_token: AccessToken::new(access_token.to_owned()),
                refresh_token: refresh_token
                    .map(|refresh_token| RefreshToken::new(refresh_token.to_owned())),
                expires_at: Timestamp::from_millisecond(expires_at)
                    .expect("timestamp should always be within the valid range"),
            })
        } else {
            None
        }
    }
}

pub struct UpdateDate<'a> {
    database: &'a Database,
}
impl<'a> UpdateDate<'a> {
    const TABLE: TableDefinition<'static, String, i64> = TableDefinition::new("last_seen_video");

    pub fn set(&self, channel_id: String, timestamp: Timestamp) {
        let write_txn = self.database.connection.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(Self::TABLE).unwrap();
            table
                .insert(channel_id, timestamp.as_millisecond())
                .unwrap();
        }
        write_txn.commit().unwrap();
    }

    pub fn get(&self, channel_id: String) -> Option<Timestamp> {
        let read_txn = self.database.connection.begin_read().unwrap();
        let table = read_txn.open_table(Self::TABLE).unwrap();

        if let Some(value) = table.get(channel_id).unwrap() {
            Some(
                Timestamp::from_millisecond(value.value())
                    .expect("stored timestamp should be valid"),
            )
        } else {
            None
        }
    }
}
