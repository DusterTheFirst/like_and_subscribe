use std::{path::PathBuf, sync::Arc};

use oauth2::RefreshToken;
use redb::{ReadableDatabase as _, TableDefinition};

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
                txn.commit().unwrap();

                db
            }),
        }
    }

    pub fn oauth(&self) -> OAuth<'_> {
        OAuth { database: self }
    }
}

pub struct OAuth<'a> {
    database: &'a Database,
}
impl<'a> OAuth<'a> {
    const TABLE: TableDefinition<'static, (), &'static str> = TableDefinition::new("oauth");

    pub fn set(&self, refresh_token: RefreshToken) {
        let write_txn = self.database.connection.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(Self::TABLE).unwrap();
            table.insert((), refresh_token.secret().as_str()).unwrap();
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

    pub fn get(&self) -> Option<RefreshToken> {
        let read_txn = self.database.connection.begin_read().unwrap();
        let table = read_txn.open_table(Self::TABLE).unwrap();

        if let Some(value) = table.get(()).unwrap() {
            let refresh_token = value.value();

            Some(RefreshToken::new(refresh_token.to_owned()))
        } else {
            None
        }
    }
}
