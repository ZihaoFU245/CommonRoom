//! Offline transfer delegates all database mechanics to the database owner.
use super::*;
pub(crate) use database::MigrationReport;

impl Engine {
    pub(crate) fn migrate_sqlite(
        source: &Path,
        destination: &str,
    ) -> Result<MigrationReport, EngineError> {
        database::transfer_sqlite(source, destination, |db| {
            validate_metadata(db)
                .map_err(|error| database::Error::InvalidData(error.message().into()))
        })
        .map_err(EngineError::from)
    }
}

fn validate_metadata(db: &Connection) -> Result<(), EngineError> {
    let data = load_data(db)?;
    crate::bots::models::validate(&data).map_err(EngineError::unavailable)?;
    authorization::CompiledPolicy::compile(&data).map_err(EngineError::unavailable)?;
    load_read_positions(db)?;
    Ok(())
}
