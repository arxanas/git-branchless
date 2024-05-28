use std::cmp::Ordering;
use std::convert::Infallible;
use std::io;
use std::ops::RangeInclusive;

use indexmap::IndexMap;
use scm_bisect::search;

type Node = isize;

#[derive(Debug)]
struct Graph;

impl search::Graph for Graph {
    type Node = Node;

    type Error = Infallible;

    fn is_ancestor(
        &self,
        ancestor: Self::Node,
        descendant: Self::Node,
    ) -> Result<bool, Self::Error> {
        // Note that a node is always considered an ancestor of itself.
        Ok(ancestor <= descendant)
    }
}

#[derive(Debug)]
struct Strategy {
    range: RangeInclusive<Node>,
}

impl search::Strategy<Graph> for Strategy {
    type Error = Infallible;

    fn midpoints(
        &self,
        _graph: &Graph,
        bounds: &search::Bounds<Node>,
        _statuses: &IndexMap<Node, search::Status>,
    ) -> Result<Vec<Node>, Self::Error> {
        let search::Bounds {
            success: success_bounds,
            failure: failure_bounds,
        } = bounds;
        let lower_bound = success_bounds
            .iter()
            .max()
            .copied()
            .unwrap_or_else(|| self.range.start() - 1);
        let upper_bound = failure_bounds
            .iter()
            .min()
            .copied()
            .unwrap_or_else(|| self.range.end() + 1);
        let midpoint = if lower_bound < upper_bound - 1 {
            (lower_bound + upper_bound) / 2
        } else {
            return Ok(Vec::new());
        };
        assert!(self.range.contains(&midpoint));
        Ok(vec![midpoint])
    }
}

fn play<E>(mut read_input: impl FnMut(isize) -> Result<Ordering, E>) -> Result<Option<isize>, E> {
    let search_range = 0..=100;
    let mut search = search::Search::new_with_nodes(Graph, search_range.clone());
    let strategy = Strategy {
        range: search_range,
    };

    let result = loop {
        let guess = {
            let mut guess = search.search(&strategy).unwrap();
            match guess.next_to_search.next() {
                Some(guess) => guess.unwrap(),
                None => {
                    break None;
                }
            }
        };
        let input = read_input(guess)?;
        match input {
            Ordering::Less => search.notify(guess, search::Status::Failure).unwrap(),
            Ordering::Greater => search.notify(guess, search::Status::Success).unwrap(),
            Ordering::Equal => {
                break Some(guess);
            }
        }
    };
    Ok(result)
}

fn main() -> io::Result<()> {
    println!("Think of a number between 0 and 100.");
    let result = play(|guess| -> io::Result<_> {
        println!("Is your number {guess}? [<=>]");
        let result = loop {
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            match input.trim() {
                "<" => break Ordering::Less,
                "=" => break Ordering::Equal,
                ">" => break Ordering::Greater,
                _ => println!("Please enter '<', '=', or '>'."),
            }
        };
        Ok(result)
    })?;
    match result {
        Some(result) => println!("I win! Your number was: {result}"),
        None => println!("I give up!"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        #[test]
        fn test_no_crashes_on_valid_input(inputs: Vec<Ordering>) {
            struct Exit;
            let mut iter = inputs.into_iter();
            let _: Result<Option<isize>, Exit> = play(move |_| iter.next().ok_or(Exit));
        }

        #[test]
        fn test_finds_number(input in 0..=100_isize) {
            let result = play(|guess| -> Result<Ordering, Infallible> { Ok(input.cmp(&guess)) });
            assert_eq!(result, Ok(Some(input)));
        }
    }
}
