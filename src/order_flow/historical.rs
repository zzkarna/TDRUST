use std::{io::Read, path::Path};

use dbn::{
    MboMsg, Schema,
    decode::{DbnDecoder, DbnMetadata, DecodeRecordRef},
};

use super::{
    cache::MboCacheEntry,
    mbo::{MboAction, MboEvent, MboEventSource, MboFlags, MboPrice, MboSide, MboSourceInfo},
};

trait DbnMboReader {
    fn schema(&self) -> Option<Schema>;
    fn decode_mbo(&mut self) -> Result<Option<MboMsg>, String>;
}

impl<R> DbnMboReader for DbnDecoder<R>
where
    R: Read,
{
    fn schema(&self) -> Option<Schema> {
        self.metadata().schema
    }

    fn decode_mbo(&mut self) -> Result<Option<MboMsg>, String> {
        while let Some(record) = self
            .decode_record_ref()
            .map_err(|error| error.to_string())?
        {
            if let Some(mbo) = record.get::<MboMsg>() {
                return Ok(Some(mbo.clone()));
            }
        }

        Ok(None)
    }
}

pub struct HistoricalDbnMboSource {
    source: MboSourceInfo,
    reader: Box<dyn DbnMboReader>,
}

impl HistoricalDbnMboSource {
    pub fn from_cache_entry(entry: &MboCacheEntry) -> Result<Self, String> {
        let path = &entry.raw_path;
        let decoder = open_decoder(path)?;
        validate_mbo_metadata(decoder.schema())?;
        Ok(Self {
            source: MboSourceInfo {
                dataset: entry.dataset.clone(),
                schema: entry.schema.clone(),
                symbol: entry.symbol.clone(),
                stype_in: entry.stype_in.clone(),
                stype_out: entry.stype_out.clone(),
            },
            reader: decoder,
        })
    }
}

impl MboEventSource for HistoricalDbnMboSource {
    fn source_info(&self) -> &MboSourceInfo {
        &self.source
    }

    fn next_batch(&mut self, max_events: usize) -> Result<Vec<MboEvent>, String> {
        let mut events = Vec::with_capacity(max_events);
        if max_events == 0 {
            return Ok(events);
        }

        while events.len() < max_events {
            let Some(record) = self.reader.decode_mbo()? else {
                break;
            };
            events.push(mbo_msg_to_event(&self.source, &record));
        }

        Ok(events)
    }
}

fn open_decoder(path: &Path) -> Result<Box<dyn DbnMboReader>, String> {
    if path.extension().and_then(|value| value.to_str()) == Some("zst") {
        DbnDecoder::from_zstd_file(path)
            .map(|decoder| Box::new(decoder) as Box<dyn DbnMboReader>)
            .map_err(|error| error.to_string())
    } else {
        DbnDecoder::from_file(path)
            .map(|decoder| Box::new(decoder) as Box<dyn DbnMboReader>)
            .map_err(|error| error.to_string())
    }
}

fn validate_mbo_metadata(schema: Option<Schema>) -> Result<(), String> {
    match schema {
        Some(Schema::Mbo) => Ok(()),
        Some(other) => Err(format!("expected mbo schema, found {other:?}")),
        None => Err("expected mbo schema, found mixed or unspecified schema".to_string()),
    }
}

fn mbo_msg_to_event(source: &MboSourceInfo, record: &MboMsg) -> MboEvent {
    MboEvent {
        source: source.clone(),
        ts_recv: record.ts_recv,
        ts_event: record.hd.ts_event,
        ts_in_delta: record.ts_in_delta,
        sequence: record.sequence,
        rtype: record.hd.rtype,
        publisher_id: record.hd.publisher_id,
        instrument_id: record.hd.instrument_id,
        action: MboAction::from_code(record.action as u8),
        side: MboSide::from_code(record.side as u8),
        price: MboPrice::from_fixed(record.price),
        size: record.size,
        channel_id: record.channel_id,
        order_id: record.order_id,
        flags: MboFlags::new(record.flags.raw()),
        symbol: Some(source.symbol.clone()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dbn::{FlagSet, MboMsg, RecordHeader, enums::rtype};

    use super::mbo_msg_to_event;
    use crate::order_flow::{
        HistoricalDbnMboSource, MboAction, MboCacheEntry, MboEventSource, MboFlags, MboPrice,
        MboSide, MboSourceInfo,
    };

    #[test]
    fn dbn_mbo_message_maps_to_normalized_event() {
        let source = MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id");
        let message = MboMsg {
            hd: RecordHeader::new::<MboMsg>(rtype::MBO, 7, 42, 123_000),
            order_id: 99,
            price: 18_250_250_000_000,
            size: 3,
            flags: FlagSet::new(MboFlags::LAST | MboFlags::SNAPSHOT),
            channel_id: 2,
            action: b'A' as i8,
            side: b'B' as i8,
            ts_recv: 124_000,
            ts_in_delta: -1000,
            sequence: 77,
        };

        let event = mbo_msg_to_event(&source, &message);

        assert_eq!(event.source.symbol, "NQ.FUT");
        assert_eq!(event.instrument_id, 42);
        assert_eq!(event.publisher_id, 7);
        assert_eq!(event.action, MboAction::Add);
        assert_eq!(event.side, MboSide::Bid);
        assert_eq!(event.price, MboPrice::from_fixed(18_250_250_000_000));
        assert_eq!(event.size, 3);
        assert!(event.flags.is_last());
        assert!(event.flags.is_snapshot());
    }

    #[test]
    #[ignore = "requires local ignored NQ.FUT MBO DBN cache"]
    fn local_nq_test_cache_scans_first_mbo_events() {
        let entry = MboCacheEntry {
            dataset: "GLBX.MDP3".to_string(),
            schema: "mbo".to_string(),
            symbol: "NQ.FUT".to_string(),
            stype_in: "parent".to_string(),
            stype_out: "instrument_id".to_string(),
            start: Some("2024-11-29T00:00:00Z".to_string()),
            end_exclusive: Some("2024-12-03T00:00:00Z".to_string()),
            raw_path: PathBuf::from(
                "E-mini/data/raw/NQ-FUT/mbo/GLBX-MDP3_NQ-FUT_mbo_parent_20241129T000000Z_20241203T000000Z.dbn.zst",
            ),
            estimated_record_count: Some(21_322_665),
            estimated_cost_usd: Some(2.001714549959),
            estimated_billable_size_bytes: Some(1_194_069_240),
            bytes_on_disk: Some(387_945_589),
        };

        assert!(
            entry.raw_path.exists(),
            "local ignored DBN cache is missing"
        );
        let mut source = HistoricalDbnMboSource::from_cache_entry(&entry).expect("DBN source");
        let batch = source.next_batch(5).expect("first batch");

        assert!(!batch.is_empty());
        assert!(batch.iter().all(|event| event.source.schema == "mbo"));
        assert!(
            batch
                .iter()
                .all(|event| event.source.dataset == "GLBX.MDP3")
        );
    }
}
