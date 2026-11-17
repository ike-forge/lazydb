//! The vocabulary every engine shares; driver types stop at the adapter
//! (architecture.md §5).

pub mod class_option;
pub mod error;
pub mod ids;
pub mod input;
pub mod page;
pub mod schema;
pub mod statement;
pub mod value;

pub use error::DbError;
pub use ids::{ConnId, JobId, QueryId};
pub use input::{Key, KeyPress, Modifiers};
pub use page::{Page, PagingState, Row};
pub use schema::{
    Catalog, Column, ColumnDefault, ColumnKind, Constraint, ConstraintKind, ForeignKey, Index,
    KeyColumn, KeyRole, Options, PrimaryKey, RelationKind, RelationRef, Schema, SchemaKind, Server,
    SortOrder, Table,
};
pub use statement::{Statement, StatementKind, UpdatedRow};
pub use value::{SortKey, Value};
