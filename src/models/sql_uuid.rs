use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;
use uuid::fmt::Hyphenated;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SqlUuid(pub Uuid);

impl SqlUuid {
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }
    
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for SqlUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl FromStr for SqlUuid {
    type Err = uuid::Error;
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::from_str(s).map(Self)
    }
}

impl From<Uuid> for SqlUuid {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<SqlUuid> for Uuid {
    fn from(sql_uuid: SqlUuid) -> Self {
        sql_uuid.0
    }
}

impl PartialEq<Uuid> for SqlUuid {
    fn eq(&self, other: &Uuid) -> bool {
        &self.0 == other
    }
}

impl PartialEq<SqlUuid> for Uuid {
    fn eq(&self, other: &SqlUuid) -> bool {
        self == &other.0
    }
}

impl Serialize for SqlUuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for SqlUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Uuid::deserialize(deserializer).map(Self)
    }
}

impl sqlx::Type<sqlx::sqlite::Sqlite> for SqlUuid {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <Hyphenated as sqlx::Type<sqlx::sqlite::Sqlite>>::type_info()
    }
}

impl<'q> sqlx::Encode<'q, sqlx::sqlite::Sqlite> for SqlUuid {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
    ) -> sqlx::encode::IsNull {
        <Hyphenated as sqlx::Encode<'q, sqlx::sqlite::Sqlite>>::encode_by_ref(
            &self.0.hyphenated(),
            buf,
        )
    }
}

impl sqlx::Decode<'_, sqlx::sqlite::Sqlite> for SqlUuid {
    fn decode(
        value: <sqlx::sqlite::Sqlite as sqlx::database::HasValueRef<'_>>::ValueRef,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let hyphenated = <Hyphenated as sqlx::Decode<sqlx::sqlite::Sqlite>>::decode(value)?;
        let uuid_str = hyphenated.to_string();
        Uuid::parse_str(&uuid_str)
            .map(SqlUuid)
            .map_err(|e| Box::new(e) as sqlx::error::BoxDynError)
    }
}
