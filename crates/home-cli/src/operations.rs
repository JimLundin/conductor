use std::path::PathBuf;

use home::operations::Handle;
use project_runtime::operations::Operations;

pub fn status(args: &[String]) -> Option<(String, u8)> {
    let query = home::operations::status_query(args)?;
    let answer = query
        .map_err(String::from)
        .and_then(|(handle, dir)| read(handle, dir.map(PathBuf::from)));
    Some(answer.map_or_else(
        |error| (format!("home: {error}\n"), 1),
        |text| (format!("{text}\n"), 0),
    ))
}

fn read(handle: Handle, directory: Option<PathBuf>) -> Result<String, String> {
    let data = project_runtime::data_dir_or_default(directory)
        .map_err(|error| error.to_string())?;
    let store = Operations::home(&data).map_err(|error| error.to_string())?;
    let record = store
        .status(handle)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| String::from("unknown operation"))?;
    serde_json::to_string_pretty(&record).map_err(|error| error.to_string())
}
