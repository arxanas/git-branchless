pub fn trim_lines(output: String) -> String {
    output
        .lines()
        .flat_map(|line| vec![line.trim_end(), "\n"].into_iter())
        .collect()
}
