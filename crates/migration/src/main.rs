use std::env;

use color_eyre::eyre::Context;
use lexopt::ValueExt;
use migration::Migrator;
use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    tracing_subscriber::fmt().init();

    let database_url = env::var("DATABASE_URL").wrap_err("DATABASE_URL should be set")?;

    let connection = sea_orm::Database::connect(&database_url).await?;

    let mut arg_parser = lexopt::Parser::from_env();
    while let Some(argument) = arg_parser.next().wrap_err("failed to parse arguments")? {
        match argument {
            lexopt::Arg::Short(_) => todo!(),
            lexopt::Arg::Long(_) => todo!(),
            lexopt::Arg::Value(os_string) => {
                match os_string
                    .string()
                    .wrap_err("invalid utf-8 in arguments")?
                    .as_str()
                {
                    "fresh" => Migrator::fresh(&connection).await?,
                    "refresh" => Migrator::refresh(&connection).await?,
                    "reset" => Migrator::reset(&connection).await?,
                    "status" => Migrator::status(&connection).await?,
                    "up" => unimplemented!("requires second argument"),
                    "down" => unimplemented!("requires second argument"),
                    _ => unimplemented!(),
                }
            }
        }
    }

    Ok(())
}
