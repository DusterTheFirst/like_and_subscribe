use sea_orm::{DeriveActiveEnum, EnumIter};

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Text")]
pub enum SubscriptionAction {
    #[sea_orm(string_value = "subscribe")]
    Subscribe,
    #[sea_orm(string_value = "unsubscribe")]
    Unsubscribe,
}
