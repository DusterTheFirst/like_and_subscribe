use sea_orm::{
    ColumnType, TryGetable, Value,
    sea_query::{ArrayType, Nullable, ValueType, ValueTypeErr},
};

/// Storage type for a [`jiff::Timestamp`] which will store the timesamp as an
/// BIGINTEGER representing miliseconds since the UNIX epoch
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct JiffTimestampMilliseconds(pub jiff::Timestamp);

impl From<JiffTimestampMilliseconds> for Value {
    fn from(JiffTimestampMilliseconds(timestamp): JiffTimestampMilliseconds) -> Self {
        Value::BigInt(Some(timestamp.as_millisecond()))
    }
}

impl Nullable for JiffTimestampMilliseconds {
    fn null() -> Value {
        Value::BigInt(None)
    }
}

impl ValueType for JiffTimestampMilliseconds {
    fn try_from(v: Value) -> Result<Self, ValueTypeErr> {
        match v {
            Value::BigInt(Some(x)) => jiff::Timestamp::from_millisecond(x)
                .map_err(|_e| ValueTypeErr)
                .map(JiffTimestampMilliseconds),
            _ => Err(ValueTypeErr),
        }
    }

    fn type_name() -> String {
        "JiffTimestampMilliseconds".to_owned()
    }

    fn array_type() -> sea_orm::sea_query::ArrayType {
        ArrayType::BigInt
    }

    fn column_type() -> sea_orm::ColumnType {
        ColumnType::BigInteger
    }
}

impl TryGetable for JiffTimestampMilliseconds {
    fn try_get_by<I: sea_orm::ColIdx>(
        res: &sea_orm::QueryResult,
        index: I,
    ) -> Result<Self, sea_orm::TryGetError> {
        i64::try_get_by(res, index).and_then(|int| {
            jiff::Timestamp::from_millisecond(int)
                .map_err(|e| {
                    sea_orm::TryGetError::DbErr(sea_orm::DbErr::TryIntoErr {
                        from: "i64",
                        into: "jiff::Timestamp",
                        source: Box::new(e),
                    })
                })
                .map(JiffTimestampMilliseconds)
        })
    }
}

/// Storage type for a [`jiff::SignedDuration`] which will store the duration as an
/// BIGINTEGER representing the duration in seconds
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct JiffSignedDurationSeconds(pub jiff::SignedDuration);

impl From<JiffSignedDurationSeconds> for Value {
    fn from(JiffSignedDurationSeconds(duration): JiffSignedDurationSeconds) -> Self {
        Value::BigInt(Some(duration.as_secs()))
    }
}

impl Nullable for JiffSignedDurationSeconds {
    fn null() -> Value {
        Value::BigInt(None)
    }
}

impl ValueType for JiffSignedDurationSeconds {
    fn try_from(v: Value) -> Result<Self, ValueTypeErr> {
        match v {
            Value::BigInt(Some(x)) => Ok(JiffSignedDurationSeconds(
                jiff::SignedDuration::from_secs(x),
            )),
            _ => Err(ValueTypeErr),
        }
    }

    fn type_name() -> String {
        "JiffSignedDurationSeconds".to_owned()
    }

    fn array_type() -> sea_orm::sea_query::ArrayType {
        ArrayType::BigInt
    }

    fn column_type() -> sea_orm::ColumnType {
        ColumnType::BigInteger
    }
}

impl TryGetable for JiffSignedDurationSeconds {
    fn try_get_by<I: sea_orm::ColIdx>(
        res: &sea_orm::QueryResult,
        index: I,
    ) -> Result<Self, sea_orm::TryGetError> {
        i64::try_get_by(res, index)
            .map(|int| JiffSignedDurationSeconds(jiff::SignedDuration::from_secs(int)))
    }
}
