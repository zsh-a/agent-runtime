use super::*;

pub(super) fn decode_schema_json<T: DeserializeOwned>(
    body: &Bytes,
    schema_json: &str,
    schema_name: &str,
) -> std::result::Result<T, Box<Response>> {
    let body = if body.is_empty() {
        b"{}"
    } else {
        body.as_ref()
    };
    let value = match serde_json::from_slice::<Value>(body) {
        Ok(value) => value,
        Err(err) => {
            return Err(Box::new(http_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                format!("request body is not valid JSON: {err}"),
            )));
        }
    };
    let schema = match crate::schema_validation::parse_schema(schema_json) {
        Ok(schema) => schema,
        Err(err) => {
            return Err(Box::new(http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "schema_load_failed",
                format!("failed to load {schema_name} schema: {err}"),
            )));
        }
    };
    let errors = match crate::schema_validation::validation_errors(&schema, &value) {
        Ok(errors) => errors,
        Err(err) => {
            return Err(Box::new(http_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "schema_compile_failed",
                format!("failed to compile {schema_name} schema: {err}"),
            )));
        }
    };
    if !errors.is_empty() {
        return Err(Box::new(http_error(
            StatusCode::BAD_REQUEST,
            "schema_validation_failed",
            format!(
                "{schema_name} request failed schema validation: {}",
                errors.join("; ")
            ),
        )));
    }

    serde_json::from_value(value).map_err(|err| {
        Box::new(http_error(
            StatusCode::BAD_REQUEST,
            "request_decode_failed",
            format!("failed to decode {schema_name} request: {err}"),
        ))
    })
}

pub(super) fn http_error(status: StatusCode, code: &str, err: impl std::fmt::Display) -> Response {
    warn!(
        status = status.as_u16(),
        code,
        error = %err,
        "HTTP request failed",
    );
    (
        status,
        Json(HttpErrorBody {
            code: code.to_owned(),
            message: err.to_string(),
            details: None,
        }),
    )
        .into_response()
}

pub(super) fn http_report_error(status: StatusCode, code: &str, err: miette::Report) -> Response {
    if let Some(error) = err.downcast_ref::<PolicyDeniedError>() {
        return http_error_body(
            StatusCode::FORBIDDEN,
            "policy_denied",
            error.message.clone(),
            Some(error.details.clone()),
        );
    }
    http_error(status, code, err)
}

pub(super) fn http_error_body(
    status: StatusCode,
    code: &str,
    message: String,
    details: Option<Value>,
) -> Response {
    warn!(
        status = status.as_u16(),
        code,
        error = %message,
        "HTTP request failed",
    );
    (
        status,
        Json(HttpErrorBody {
            code: code.to_owned(),
            message,
            details,
        }),
    )
        .into_response()
}
