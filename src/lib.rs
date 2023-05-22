use regex::Regex;
use once_cell::sync::OnceCell;
use eyre::ContextCompat;
use serde::Deserialize;
use serde_json::Value;

use fluvio_smartmodule::{
    smartmodule, Result, Record, RecordData,
    dataplane::smartmodule::{
        SmartModuleExtraParams, SmartModuleInitError
    },
    eyre
};

static OPS: OnceCell<Vec<Operation>> = OnceCell::new();
const PARAM_NAME: &str = "spec";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Replace(Replace)
}

#[derive(Clone, Debug, Deserialize)]
struct Replace {
    regex: String,
    with: String,

    #[serde(skip)]
    re: Option<Regex>,
}

impl Operation {
    pub fn get_re(&self) -> &Option<Regex> {
        match self {
            Operation::Replace(r) => &r.re
        }
    }

    pub fn clone_with_regex(&self) -> Result<Operation> {
        let op = match self {
            Operation::Replace(r) => {
                let mut new = r.clone();
                new.re = Some(Regex::new(&r.regex.as_str())?);
                Operation::Replace(new)
            }
        };
        Ok(op)
    }

    pub fn run_regex(&self, text: &String) -> String {
        match self {
            Operation::Replace(r) => {
                let regex = r.re.as_ref().unwrap();
                regex.replace_all(text,  &r.with).to_string()
            }
        }
    }
}

/// Parse input paramters
fn get_params(params: SmartModuleExtraParams) -> Result<Vec<Operation>> {
    if let Some(raw_spec) = params.get(PARAM_NAME) {
        match serde_json::from_str(raw_spec) {
            Ok(operations) => {
                Ok(operations)
            }
            Err(err) => {
                eprintln!("unable to parse spec from params: {err:?}");
                Err(eyre!("cannot parse `spec` param: {:#?}", err))
            }
        }
    } else {
        Err(SmartModuleInitError::MissingParam(PARAM_NAME.to_string()).into())
    }
}

/// Loop over operations and compile regex
fn compile_regex(operations: Vec<Operation>) -> Result<Vec<Operation>> {
    let mut result: Vec<Operation> = vec![];

    let mut iter = operations.into_iter();
    while let Some(op) = iter.next() {
        result.push(op.clone_with_regex()?);
    }

    Ok(result)
}

/// Traverse the regex list, compute regex, and collect output
fn apply_regex_ops_to_json_record(record: &Record, ops: &Vec<Operation>) -> Result<Value> {
    let data_str: &str = std::str::from_utf8(record.value.as_ref())?;
    let mut data = data_str.to_string();

    let mut iter = ops.into_iter();
    while let Some(op) = iter.next() {
        if op.get_re().is_some() {
            data = op.run_regex(&data);
        }
    }

    Ok(serde_json::from_str(data.as_str())?)
}    

#[smartmodule(map)]
pub fn map(record: &Record) -> Result<(Option<RecordData>, RecordData)> {
    let key = record.key.clone();
    let ops = OPS.get().wrap_err("regex operations not initialized")?;

    let result = apply_regex_ops_to_json_record(record, ops)?;
    Ok((key, serde_json::to_string(&result)?.into()))
}

#[smartmodule(init)]
fn init(params: SmartModuleExtraParams) -> Result<()> {
    let ops = get_params(params)?;

    let regex_ops = compile_regex(ops)?;
    OPS.set(regex_ops).expect("regex operations already initialized");

    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    static INPUT: &str = r#"{
        "description": "Independence High School",
        "class": "2025-A",
        "students": [
          {
              "first": "Abby",
              "last": "Hardy",
              "address": "285 LA PALA DR APT 2343, SAN JOSE CA 95127",
              "ssn": "123-45-6789"
          },
          {
              "first": "Bob",
              "last": "Newmal",
              "address": "21 E TRIMBLE RD, Santa Clara CA 95347",
              "ssn": "987-65-4321"
          },
          {
              "first": "Cindy",
              "last": "Hall",
              "address": "1601 PRIME PL, Milpitas CA 95344",
              "ssn": "999-88-7777"
          }
        ]
    }"#;

    #[test]
    fn test_compile_regex() {
        let params = vec![
            Operation::Replace(Replace {
                regex: r"\d{3}-\d{2}-\d{4}".to_owned(),
                with: "***-**-****".to_owned(),
                re: None
            })
        ];

        let result = compile_regex(params);
        assert!(result.is_ok());
        for r in result.unwrap() {
            assert!(r.get_re().is_some());
        }
    }

    #[test]
    fn run_regex_test() {
        // Replace exact
        let input = r"123-45-6789".to_owned();
        let regex = r"\d{3}-\d{2}-\d{4}";
        let op = Operation::Replace(Replace {
            regex: regex.to_owned(),
            with: "***-**-****".to_owned(),
            re: Some(Regex::new(regex).unwrap())
        });
        let expected = "***-**-****".to_owned();

        let result = op.run_regex(&input);
        assert_eq!(result, expected);

        // Replace subset
        let input = r"Alice Jackson, ssn 123-45-6789, location: NY".to_owned();
        let regex = r"\d{3}-\d{2}-\d{4}";
        let op = Operation::Replace(Replace {
            regex: regex.to_owned(),
            with: "***-**-****".to_owned(),
            re: Some(Regex::new(regex).unwrap())
        });
        let expected = "Alice Jackson, ssn ***-**-****, location: NY".to_owned();

        let result = op.run_regex(&input);
        assert_eq!(result, expected);

        // Replace multiple
        let input = r"Alice, ssn 123-45-6789, Jack, ssn 987-65-4321".to_owned();
        let regex = r"\d{3}-\d{2}-\d{4}";
        let op = Operation::Replace(Replace {
            regex: regex.to_owned(),
            with: "***-**-****".to_owned(),
            re: Some(Regex::new(regex).unwrap())
        });
        let expected = "Alice, ssn ***-**-****, Jack, ssn ***-**-****".to_owned();

        let result = op.run_regex(&input);
        assert_eq!(result, expected);

        // Replace address
        let input = r#""address": "285 LA PALA DR APT 2343, SAN JOSE CA 95127""#.to_owned();
        let regex = r#"(?P<first>"address":\s+\")([\w\d\s]+),"#;
        let op = Operation::Replace(Replace {
            regex: regex.to_owned(),
            with: "${first}...".to_owned(),
            re: Some(Regex::new(regex).unwrap())
        });
        let expected = r#""address": "... SAN JOSE CA 95127""#.to_owned();

        let result = op.run_regex(&input);
        assert_eq!(result, expected);

        // Replace none
        let input = r"not a match".to_owned();
        let regex = r"\d{3}-\d{2}-\d{4}";
        let op = Operation::Replace(Replace {
            regex: regex.to_owned(),
            with: "***-**-****".to_owned(),
            re: Some(Regex::new(regex).unwrap())
        });
        let expected = r"not a match".to_owned();

        let result = op.run_regex(&input);
        assert_eq!(result, expected);
    }

    #[test]
    fn apply_regex_ops_to_json_record_tests() {
        static EXPECTED: &str = r#"{
            "description": "Independence High School",
            "class": "2025-A",
            "students": [
              {
                  "first": "Abby",
                  "last": "Hardy",
                  "ssn": "***-**-****",
                  "address": "... SAN JOSE CA 95127"
              },
              {
                  "first": "Bob",
                  "last": "Newmal",
                  "ssn": "***-**-****",
                  "address": "... Santa Clara CA 95347"
              },
              {
                  "first": "Cindy",
                  "last": "Hall",
                  "ssn": "***-**-****",
                  "address": "... Milpitas CA 95344"
              }
            ]
        }"#;
        let spec = vec![
            Operation::Replace(Replace {
                regex: r"\d{3}-\d{2}-\d{4}".to_owned(),
                with: "***-**-****".to_owned(),
                re: None
            }),
            Operation::Replace(Replace {
                regex:  r#"(?P<first>"address":\s+\")([\w\d\s]+),"#.to_owned(),
                with: "${first}...".to_owned(),
                re: None
            })
        ];

        let record = Record::new(INPUT);
        let ops = compile_regex(spec).unwrap();
        let result = apply_regex_ops_to_json_record(&record, &ops).unwrap();

        let expected_value:Value = serde_json::from_str(EXPECTED).unwrap();
        assert_eq!(result, expected_value);
    }

}