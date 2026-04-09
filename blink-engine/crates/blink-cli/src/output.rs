//! Output formatting helpers — tables (default) and JSON.

use colored::Colorize;
use comfy_table::{Table, Cell, Color, Attribute, ContentArrangement};
use serde_json::Value;

use crate::OutputFormat;

/// Print a JSON value as either a pretty table (best-effort) or raw JSON.
pub fn print_value(val: &Value, fmt: &OutputFormat) {
    match fmt {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(val).unwrap_or_default());
        }
        OutputFormat::Table => {
            // If the value is an array, print as a table.
            if let Some(arr) = val.as_array() {
                print_array_table(arr);
            } else if let Some(obj) = val.as_object() {
                print_kv_table(obj);
            } else {
                println!("{val}");
            }
        }
    }
}

/// Print a key-value object as a two-column table.
pub fn print_kv_table(obj: &serde_json::Map<String, Value>) {
    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("Key").add_attribute(Attribute::Bold).fg(Color::Cyan),
            Cell::new("Value").add_attribute(Attribute::Bold).fg(Color::Cyan),
        ]);
    for (k, v) in obj {
        let val_str = match v {
            Value::String(s) => s.clone(),
            Value::Null      => "—".dimmed().to_string(),
            other            => other.to_string(),
        };
        table.add_row(vec![k.as_str(), val_str.as_str()]);
    }
    println!("{table}");
}

/// Print an array of objects as a generic table.
pub fn print_array_table(arr: &[Value]) {
    if arr.is_empty() {
        println!("{}", "(no results)".dimmed());
        return;
    }
    // Collect column names from the first object.
    let Some(first) = arr.first().and_then(|v| v.as_object()) else {
        for v in arr { println!("{v}"); }
        return;
    };

    let cols: Vec<String> = first.keys().cloned().collect();
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(
        cols.iter()
            .map(|c| Cell::new(c).add_attribute(Attribute::Bold).fg(Color::Cyan))
            .collect::<Vec<_>>(),
    );

    for item in arr {
        if let Some(obj) = item.as_object() {
            let cells: Vec<String> = cols.iter().map(|c| {
                let v = obj.get(c).unwrap_or(&Value::Null);
                match v {
                    Value::String(s) => s.clone(),
                    Value::Null      => "—".to_string(),
                    other            => other.to_string(),
                }
            }).collect();
            table.add_row(cells);
        }
    }
    println!("{table}");
}

/// Colour a P&L value: green if positive, red if negative.
pub fn format_pnl(pnl: f64) -> String {
    let s = format!("{:+.4}", pnl);
    if pnl > 0.0 { s.green().to_string() }
    else if pnl < 0.0 { s.red().to_string() }
    else { s }
}

/// Friendly percentage string.
pub fn pct(v: f64) -> String {
    format!("{:.1}%", v)
}
