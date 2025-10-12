use std::{path::PathBuf, sync::Arc};

use jiff::Timestamp;
use oauth2::{AccessToken, RefreshToken};
use redb::{ReadableDatabase as _, TableDefinition};

use crate::oauth::Authentication;

#[derive(Clone)]
pub struct Database {
    connection: Arc<redb::Database>,
    earliest_update: Timestamp,
}

impl Database {
    pub async fn create(
        database_path: PathBuf,
        earliest_update: Timestamp,
    ) -> Result<Self, redb::Error> {
        Ok(Self {
            connection: Arc::new(
                tokio::task::spawn_blocking(|| {
                    let db = redb::Database::create(database_path)?;

                    let txn = db.begin_write()?;
                    txn.open_table(OAuth::TABLE)?;
                    txn.open_table(UpdateDate::TABLE)?;
                    txn.commit()?;

                    Ok::<_, redb::Error>(db)
                })
                .await
                .unwrap()?,
            ),
            earliest_update,
        })
    }

    pub fn oauth(&self) -> OAuth<'_> {
        OAuth { database: self }
    }

    pub fn update_date(&self) -> UpdateDate<'_> {
        UpdateDate { database: self }
    }
}

type OAuthStorage<'s> = (&'s str, &'s str, i64);
pub struct OAuth<'a> {
    database: &'a Database,
}
impl<'a> OAuth<'a> {
    const TABLE: TableDefinition<'static, (), OAuthStorage<'static>> =
        TableDefinition::new("oauth");

    pub async fn set(&self, auth: Authentication) -> Result<(), redb::Error> {
        let database = self.database.clone();

        tokio::task::spawn_blocking(move || {
            let write_txn = database.connection.begin_write()?;
            {
                let mut table = write_txn.open_table(Self::TABLE)?;
                table.insert(
                    (),
                    &(
                        auth.access_token.secret().as_str(),
                        auth.refresh_token.secret().as_str(),
                        auth.expires_at.as_millisecond(),
                    ),
                )?;
            }
            write_txn.commit()?;

            Ok(())
        })
        .await
        .unwrap()
    }

    pub async fn delete(&self) -> Result<(), redb::Error> {
        let database = self.database.clone();

        tokio::task::spawn_blocking(move || {
            let write_txn = database.connection.begin_write()?;
            {
                let mut table = write_txn.open_table(Self::TABLE)?;
                table.remove(())?;
            }
            write_txn.commit()?;

            Ok(())
        })
        .await
        .unwrap()
    }

    pub async fn get(&self) -> Result<Option<Authentication>, redb::Error> {
        let database = self.database.clone();

        tokio::task::spawn_blocking(move || {
            let read_txn = database.connection.begin_read()?;
            let table = read_txn.open_table(Self::TABLE)?;

            if let Some(value) = table.get(())? {
                let (access_token, refresh_token, expires_at) = value.value();

                Ok(Some(Authentication {
                    access_token: AccessToken::new(access_token.to_owned()),
                    refresh_token: RefreshToken::new(refresh_token.to_owned()),
                    expires_at: Timestamp::from_millisecond(expires_at)
                        .expect("timestamp should always be within the valid range"),
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .unwrap()
    }
}

pub struct UpdateDate<'a> {
    database: &'a Database,
}
impl<'a> UpdateDate<'a> {
    const TABLE: TableDefinition<'static, String, i64> = TableDefinition::new("last_seen_video");

    pub async fn set(&self, channel_id: String, timestamp: Timestamp) -> Result<(), redb::Error> {
        let database = self.database.clone();

        tokio::task::spawn_blocking(move || {
            let write_txn = database.connection.begin_write()?;
            {
                let mut table = write_txn.open_table(Self::TABLE)?;
                table.insert(channel_id, timestamp.as_millisecond())?;
            }
            write_txn.commit()?;

            Ok(())
        })
        .await
        .unwrap()
    }

    pub async fn get(&self, channel_id: String) -> Result<Timestamp, redb::Error> {
        let database = self.database.clone();

        tokio::task::spawn_blocking(move || {
            let read_txn = database.connection.begin_read()?;
            let table = read_txn.open_table(Self::TABLE)?;

            if let Some(value) = table.get(channel_id)? {
                Ok(Timestamp::from_millisecond(value.value())
                    .expect("stored timestamp should be valid"))
            } else {
                Ok(database.earliest_update)
            }
        })
        .await
        .unwrap()
    }
}
