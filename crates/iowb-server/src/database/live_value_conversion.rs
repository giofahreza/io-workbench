#[derive(Debug, Clone, PartialEq)]
enum PostgresArrayValue {
    Null,
    Text(String),
    Array(Vec<PostgresArrayValue>),
}

struct PostgresArrayParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> PostgresArrayParser<'a> {
    fn parse(input: &'a str) -> Option<PostgresArrayValue> {
        let normalized = if input.starts_with('[') {
            input.split_once('=').map(|(_, value)| value)?
        } else {
            input
        };
        let mut parser = Self {
            input: normalized.as_bytes(),
            offset: 0,
        };
        let value = parser.parse_array()?;
        (parser.offset == parser.input.len()).then_some(value)
    }

    fn parse_array(&mut self) -> Option<PostgresArrayValue> {
        self.consume(b'{')?;
        let mut values = Vec::new();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Some(PostgresArrayValue::Array(values));
        }
        loop {
            let value = if self.peek() == Some(b'{') {
                self.parse_array()?
            } else if self.peek() == Some(b'"') {
                PostgresArrayValue::Text(self.parse_quoted()?)
            } else {
                self.parse_unquoted()?
            };
            values.push(value);
            match self.peek()? {
                b',' | b';' => self.offset += 1,
                b'}' => {
                    self.offset += 1;
                    break;
                }
                _ => return None,
            }
        }
        Some(PostgresArrayValue::Array(values))
    }

    fn parse_quoted(&mut self) -> Option<String> {
        self.consume(b'"')?;
        let mut value = Vec::new();
        while let Some(byte) = self.peek() {
            self.offset += 1;
            match byte {
                b'"' => return String::from_utf8(value).ok(),
                b'\\' => {
                    value.push(self.peek()?);
                    self.offset += 1;
                }
                _ => value.push(byte),
            }
        }
        None
    }

    fn parse_unquoted(&mut self) -> Option<PostgresArrayValue> {
        let mut value = Vec::new();
        while let Some(byte) = self.peek() {
            if matches!(byte, b',' | b';' | b'}') {
                break;
            }
            self.offset += 1;
            if byte == b'\\' {
                value.push(self.peek()?);
                self.offset += 1;
            } else {
                value.push(byte);
            }
        }
        let value = String::from_utf8(value).ok()?;
        if value == "NULL" {
            Some(PostgresArrayValue::Null)
        } else {
            Some(PostgresArrayValue::Text(value))
        }
    }

    fn consume(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.offset += 1)
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }
}

fn parse_postgres_array(value: &str, element_type: &str) -> Value {
    PostgresArrayParser::parse(value)
        .map(|value| postgres_array_value_to_json(value, element_type))
        .unwrap_or_else(|| Value::String(value.to_string()))
}

fn postgres_array_value_to_json(value: PostgresArrayValue, element_type: &str) -> Value {
    match value {
        PostgresArrayValue::Null => Value::Null,
        PostgresArrayValue::Text(value) => postgres_array_scalar_to_json(&value, element_type),
        PostgresArrayValue::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| postgres_array_value_to_json(value, element_type))
                .collect(),
        ),
    }
}

fn postgres_array_scalar_to_json(value: &str, element_type: &str) -> Value {
    match element_type.to_ascii_uppercase().as_str() {
        "BOOL" => Value::Bool(matches!(
            value,
            "TRUE" | "t" | "true" | "y" | "yes" | "on" | "1"
        )),
        "INT2" | "INT4" | "OID" => value
            .parse::<i64>()
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|_| Value::String(value.to_string())),
        "INT8" => Value::String(value.to_string()),
        "FLOAT4" | "FLOAT8" | "NUMERIC" => value
            .parse::<f64>()
            .map(json_float_value)
            .unwrap_or(Value::Null),
        "BYTEA" => decode_postgres_bytea(value)
            .map(|value| database_buffer_value(&value))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => {
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
        }
        "DATE" => NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .and_then(|value| value.and_hms_opt(0, 0, 0))
            .map(local_naive_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMP" => NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
            .map(local_naive_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z")
            .map(|value| utc_datetime_to_iso(value.with_timezone(&Utc)))
            .map(Value::String)
            .unwrap_or(Value::Null),
        "INTERVAL" => parse_postgres_interval(value),
        "POINT" => parse_postgres_point(value).unwrap_or(Value::Null),
        _ => Value::String(value.to_string()),
    }
}

fn decode_postgres_bytea(value: &str) -> Option<Vec<u8>> {
    let encoded = value.strip_prefix("\\x")?;
    if encoded.len() % 2 != 0 {
        return None;
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let high = (chunk[0] as char).to_digit(16)?;
            let low = (chunk[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn parse_postgres_point(value: &str) -> Option<Value> {
    let (x, y) = value
        .strip_prefix('(')?
        .strip_suffix(')')?
        .split_once(',')?;
    Some(serde_json::json!({
        "x": x.parse::<f64>().ok().map(json_float_value).unwrap_or(Value::Null),
        "y": y.parse::<f64>().ok().map(json_float_value).unwrap_or(Value::Null),
    }))
}

fn parse_postgres_circle(value: &str) -> Option<Value> {
    let value = value.strip_prefix("<(")?.strip_suffix('>')?;
    let (point, radius) = value
        .rsplit_once(") ,")
        .or_else(|| value.rsplit_once("),"))?;
    let point = parse_postgres_point(&format!("({point})"))?;
    let mut object = point.as_object()?.clone();
    object.insert(
        "radius".to_string(),
        radius
            .trim()
            .parse::<f64>()
            .ok()
            .map(json_float_value)
            .unwrap_or(Value::Null),
    );
    Some(Value::Object(object))
}

fn parse_postgres_interval(value: &str) -> Value {
    let mut object = Map::new();
    let mut tokens = value.split_whitespace().peekable();
    let mut invert = false;
    while let Some(token) = tokens.next() {
        if token == "@" {
            continue;
        }
        if token.eq_ignore_ascii_case("ago") {
            invert = true;
            continue;
        }
        if token.contains(':') {
            let negative = token.starts_with('-');
            let time = token.trim_start_matches(['-', '+']);
            let mut parts = time.split(':');
            let hours = parts
                .next()
                .and_then(|part| part.parse::<i64>().ok())
                .unwrap_or(0);
            let minutes = parts
                .next()
                .and_then(|part| part.parse::<i64>().ok())
                .unwrap_or(0);
            let seconds = parts
                .next()
                .and_then(|part| part.parse::<f64>().ok())
                .unwrap_or(0.0);
            let sign = if negative { -1.0 } else { 1.0 };
            insert_nonzero_number(&mut object, "hours", sign * hours as f64);
            insert_nonzero_number(&mut object, "minutes", sign * minutes as f64);
            let whole_seconds = seconds.trunc() * sign;
            let milliseconds = (seconds.fract() * 1_000_000.0).round() / 1000.0 * sign;
            insert_nonzero_number(&mut object, "seconds", whole_seconds);
            insert_nonzero_number(&mut object, "milliseconds", milliseconds);
            continue;
        }
        let Some(unit) = tokens.next() else {
            continue;
        };
        let number = token.parse::<f64>().unwrap_or(0.0);
        let key = match unit.trim_end_matches('s') {
            "year" => "years",
            "mon" => "months",
            "day" => "days",
            "hour" => "hours",
            "min" | "minute" => "minutes",
            "sec" | "second" => "seconds",
            _ => continue,
        };
        insert_nonzero_number(&mut object, key, number);
    }
    if invert {
        for value in object.values_mut() {
            if let Some(number) = value.as_f64() {
                *value = json_float_value(-number);
            }
        }
    }
    Value::Object(object)
}

fn insert_nonzero_number(object: &mut Map<String, Value>, key: &str, value: f64) {
    if value == 0.0 {
        return;
    }
    let rounded = value.round();
    let value = if (value - rounded).abs() < f64::EPSILON {
        Value::Number((rounded as i64).into())
    } else {
        json_float_value(value)
    };
    object.insert(key.to_string(), value);
}

struct MysqlGeometryParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> MysqlGeometryParser<'a> {
    fn parse(input: &'a [u8]) -> Option<Value> {
        if input.len() < 5 {
            return None;
        }
        let mut parser = Self { input, offset: 4 };
        parser.parse_geometry()
    }

    fn parse_geometry(&mut self) -> Option<Value> {
        let little_endian = self.read_u8()? != 0;
        let geometry_type = self.read_u32(little_endian)?;
        match geometry_type {
            1 => {
                let x = self.read_f64(little_endian)?;
                let y = self.read_f64(little_endian)?;
                Some(serde_json::json!({ "x": json_float_value(x), "y": json_float_value(y) }))
            }
            2 => {
                let count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                let mut points = Vec::with_capacity(count);
                for _ in 0..count {
                    let x = self.read_f64(little_endian)?;
                    let y = self.read_f64(little_endian)?;
                    points.push(
                        serde_json::json!({ "x": json_float_value(x), "y": json_float_value(y) }),
                    );
                }
                Some(Value::Array(points))
            }
            3 => {
                let ring_count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                let mut rings = Vec::with_capacity(ring_count);
                for _ in 0..ring_count {
                    let point_count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                    let mut points = Vec::with_capacity(point_count);
                    for _ in 0..point_count {
                        let x = self.read_f64(little_endian)?;
                        let y = self.read_f64(little_endian)?;
                        points.push(serde_json::json!({ "x": json_float_value(x), "y": json_float_value(y) }));
                    }
                    rings.push(Value::Array(points));
                }
                Some(Value::Array(rings))
            }
            4..=7 => {
                let count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.parse_geometry().unwrap_or(Value::Null));
                }
                Some(Value::Array(values))
            }
            _ => None,
        }
    }

    fn read_u8(&mut self) -> Option<u8> {
        let value = *self.input.get(self.offset)?;
        self.offset += 1;
        Some(value)
    }

    fn read_u32(&mut self, little_endian: bool) -> Option<u32> {
        let bytes: [u8; 4] = self
            .input
            .get(self.offset..self.offset + 4)?
            .try_into()
            .ok()?;
        self.offset += 4;
        Some(if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn read_f64(&mut self, little_endian: bool) -> Option<f64> {
        let bytes: [u8; 8] = self
            .input
            .get(self.offset..self.offset + 8)?
            .try_into()
            .ok()?;
        self.offset += 8;
        Some(if little_endian {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }
}

fn parse_mysql_geometry(value: &[u8]) -> Option<Value> {
    MysqlGeometryParser::parse(value)
}

fn parse_mysql_vector(value: &[u8]) -> Value {
    Value::Array(
        value
            .chunks_exact(4)
            .map(|chunk| {
                let bytes: [u8; 4] = chunk.try_into().expect("four-byte vector chunk");
                json_float_value(f64::from(f32::from_le_bytes(bytes)))
            })
            .collect(),
    )
}

fn any_string(row: &AnyRow, index: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(index).ok().flatten()
}

fn sqlite_connection(connection: &StoredDatabaseConnection) -> Result<Connection> {
    let path = ensure_sqlite_connection(connection)?;
    if !path.exists() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            format!("SQLite database file does not exist: {}", path.display()),
            None,
            "SQLITE_CANTOPEN",
            "connection",
            false,
        ));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(sqlite_server_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_server_error)?;
    Ok(conn)
}
