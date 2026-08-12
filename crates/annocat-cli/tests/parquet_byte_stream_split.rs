use std::fs::{self, File};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::arrow::array::{Array, Float64Array, Int32Array};
use duckdb::arrow::datatypes::{DataType, Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::{Connection, params};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use parquet::schema::types::ColumnPath;

#[test]
fn viewer_readers_accept_byte_stream_split_numbers() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "annocat-byte-stream-split-{}-{suffix}.parquet",
        std::process::id()
    ));
    let expected = vec![
        Some(0.0),
        Some(-2.802),
        Some(0.000_000_657_073),
        Some(20.249_44),
        Some(f64::MIN_POSITIVE),
        None,
    ];
    let schema = Arc::new(Schema::new(vec![
        Field::new("ordinal", DataType::Int32, false),
        Field::new("number_value", DataType::Float64, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int32Array::from_iter_values(0..expected.len() as i32)),
            Arc::new(Float64Array::from(expected.clone())),
        ],
    )
    .unwrap();
    let number_value = ColumnPath::from("number_value");
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .set_column_dictionary_enabled(number_value.clone(), false)
        .set_column_encoding(number_value, Encoding::BYTE_STREAM_SPLIT)
        .build();
    let mut writer =
        ArrowWriter::try_new(File::create(&path).unwrap(), schema, Some(properties)).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();

    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap()).unwrap();
    assert!(
        builder.metadata().row_groups()[0].columns()[1]
            .encodings()
            .any(|encoding| encoding == Encoding::BYTE_STREAM_SPLIT)
    );
    let mut reader = builder.build().unwrap();
    let batch = reader.next().unwrap().unwrap();
    let numbers = batch
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    for (index, expected) in expected.iter().enumerate() {
        match expected {
            Some(expected) => assert_eq!(numbers.value(index).to_bits(), expected.to_bits()),
            None => assert!(numbers.is_null(index)),
        }
    }
    drop(reader);

    let connection = Connection::open_in_memory().unwrap();
    let mut statement = connection
        .prepare("SELECT number_value FROM read_parquet(?) ORDER BY ordinal")
        .unwrap();
    let actual = statement
        .query_map(params![path.to_string_lossy().as_ref()], |row| {
            row.get::<_, Option<f64>>(0)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(actual.map(f64::to_bits), expected.map(f64::to_bits));
    }

    drop(statement);
    drop(connection);
    fs::remove_file(path).unwrap();
}
