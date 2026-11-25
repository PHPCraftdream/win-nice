//! Extra JOBS lines under the table: the full command of a truncated row and
//! the machine-wide CPU ceiling of a nested job. Pure functions, no Win32.

use super::{rate, truncate, COMMAND_WIDTH};
use crate::front::jobs::{CapState, JobInfo, Nesting, Scan};

/// Longest full command line printed; the rest is cut with `...`.
const FULL_COMMAND_MAX: usize = 1024;

/// What the chain of a nested job says about the whole machine.
#[derive(Debug, PartialEq)]
enum Share {
    /// Hard caps along the chain, own cap first, each in percent.
    Ceiling(Vec<f64>),
    /// Only soft caps: they limit under contention, not a ceiling.
    NoCeiling,
    /// A link of the chain is unknown or unproven.
    Unknown,
    /// Nothing to say: not nested, or no CPU cap anywhere in the chain.
    Skip,
}

/// The lines for every job, in row order; empty when there is nothing extra.
pub(super) fn lines(scan: &Scan) -> Vec<String> {
    let mut out = Vec::new();
    for job in &scan.jobs {
        if let Some(text) = job.owner_command.as_deref() {
            if text.chars().count() > COMMAND_WIDTH {
                out.push(format!("job {} command: {}", job.id, printable(text)));
            }
        }
        if let Some(line) = share_line(job, &scan.jobs, scan.logical_cpus) {
            out.push(line);
        }
    }
    out
}

/// One line of text: control characters become spaces, the end is trimmed.
fn printable(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    truncate(flat.trim_end(), FULL_COMMAND_MAX)
}

fn share_line(job: &JobInfo, jobs: &[JobInfo], logical_cpus: u32) -> Option<String> {
    let id = job.id;
    match share(job, jobs) {
        Share::Skip => None,
        Share::Unknown => Some(format!("job {id} machine share: ?")),
        Share::NoCeiling => Some(format!(
            "job {id} machine share: no hard ceiling (only soft CPU caps in the chain)"
        )),
        Share::Ceiling(factors) => {
            let all = factors.iter().fold(100.0, |acc, f| acc * f / 100.0);
            let one = all * f64::from(logical_cpus);
            let chain: Vec<String> = factors.iter().map(|f| pct(*f)).collect();
            Some(format!(
                "job {id} machine share: at most {} all / {} one ({})",
                pct(all),
                pct(one),
                chain.join(" of ")
            ))
        }
    }
}

/// Percent with one decimal, two below 1% so a small share is not shown as 0.0%.
fn pct(value: f64) -> String {
    if value > 0.0 && value < 1.0 {
        format!("{value:.2}%")
    } else {
        format!("{value:.1}%")
    }
}

/// Walk from `job` up through its proven parents. A nested rate is a share of
/// the parent's allowance, so the ceiling is the product of the hard caps.
fn share(job: &JobInfo, jobs: &[JobInfo]) -> Share {
    if matches!(job.nesting, Nesting::TopLevel) {
        return Share::Skip;
    }
    let mut factors = Vec::new();
    let mut soft = false;
    let mut cur = job;
    for _ in 0..=jobs.len() {
        match cur.cpu {
            CapState::Unknown => return Share::Unknown,
            CapState::Uncapped => {}
            CapState::Capped(cap) if cap.hard => factors.push(rate(cap)),
            CapState::Capped(_) => soft = true,
        }
        match cur.nesting {
            Nesting::TopLevel => {
                return if !factors.is_empty() {
                    Share::Ceiling(factors)
                } else if soft {
                    Share::NoCeiling
                } else {
                    Share::Skip
                };
            }
            Nesting::Nested(parent) => match jobs.iter().find(|j| j.id == parent) {
                Some(next) => cur = next,
                None => return Share::Unknown,
            },
            Nesting::NestedUnnamed | Nesting::Unknown => return Share::Unknown,
        }
    }
    Share::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front::jobs::CpuCap;

    fn hard(rate: u32) -> CapState {
        CapState::Capped(CpuCap {
            cpu_rate: rate,
            hard: true,
        })
    }

    fn soft(rate: u32) -> CapState {
        CapState::Capped(CpuCap {
            cpu_rate: rate,
            hard: false,
        })
    }

    fn job(id: usize, cpu: CapState, nesting: Nesting) -> JobInfo {
        JobInfo {
            id,
            owner_pid: 1000 + id as u32,
            owner_image: "wnice.exe".to_string(),
            owner_command: None,
            cpu,
            limits: None,
            active_processes: None,
            nesting,
        }
    }

    fn scan(jobs: Vec<JobInfo>) -> Scan {
        Scan {
            complete: true,
            jobs,
            logical_cpus: 16,
            window_ms: 250,
            ..Scan::default()
        }
    }

    fn only(jobs: Vec<JobInfo>) -> Vec<String> {
        lines(&scan(jobs))
    }

    #[test]
    fn a_short_command_and_a_top_level_job_add_nothing() {
        let mut a = job(1, hard(1000), Nesting::TopLevel);
        a.owner_command = Some("x".repeat(COMMAND_WIDTH));
        assert!(only(vec![a]).is_empty());
    }

    #[test]
    fn a_truncated_command_is_printed_in_full_on_one_line() {
        let long = format!("wnice -c 10 -- {}", "arg ".repeat(20));
        let mut a = job(7, CapState::Uncapped, Nesting::TopLevel);
        a.owner_command = Some(long.clone());
        assert_eq!(
            only(vec![a]),
            vec![format!("job 7 command: {}", long.trim_end())]
        );
    }

    #[test]
    fn the_full_command_is_flattened_trimmed_and_capped() {
        let mut a = job(1, CapState::Uncapped, Nesting::TopLevel);
        a.owner_command = Some(format!("a\u{1b}[31m\nb {}\t", "c".repeat(40)));
        let line = only(vec![a]).remove(0);
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(!line.ends_with(' '));
        let mut b = job(2, CapState::Uncapped, Nesting::TopLevel);
        b.owner_command = Some("z".repeat(5000));
        let line = only(vec![b]).remove(0);
        let text = line.strip_prefix("job 2 command: ").unwrap();
        assert_eq!(text.chars().count(), FULL_COMMAND_MAX);
        assert!(text.ends_with("..."));
    }

    #[test]
    fn nested_share_is_the_product_of_the_hard_caps() {
        let one = vec![
            job(1, hard(1000), Nesting::TopLevel),
            job(2, hard(2000), Nesting::Nested(1)),
        ];
        assert_eq!(
            only(one),
            vec!["job 2 machine share: at most 2.0% all / 32.0% one (20.0% of 10.0%)"]
        );
        let two = vec![
            job(1, hard(5000), Nesting::TopLevel),
            job(2, hard(5000), Nesting::Nested(1)),
            job(3, hard(2000), Nesting::Nested(2)),
        ];
        assert_eq!(
            only(two),
            vec![
                "job 2 machine share: at most 25.0% all / 400.0% one (50.0% of 50.0%)",
                "job 3 machine share: at most 5.0% all / 80.0% one (20.0% of 50.0% of 50.0%)",
            ]
        );
    }

    #[test]
    fn an_uncapped_link_passes_the_ceiling_of_the_others_through() {
        let jobs = vec![
            job(1, hard(1000), Nesting::TopLevel),
            job(2, CapState::Uncapped, Nesting::Nested(1)),
        ];
        assert_eq!(
            only(jobs),
            vec!["job 2 machine share: at most 10.0% all / 160.0% one (10.0%)"]
        );
        let none = vec![
            job(1, CapState::Uncapped, Nesting::TopLevel),
            job(2, CapState::Uncapped, Nesting::Nested(1)),
        ];
        assert!(only(none).is_empty());
    }

    #[test]
    fn a_tiny_share_keeps_two_decimals() {
        let jobs = vec![
            job(1, hard(100), Nesting::TopLevel),
            job(2, hard(100), Nesting::Nested(1)),
        ];
        assert_eq!(
            only(jobs),
            vec!["job 2 machine share: at most 0.01% all / 0.16% one (1.0% of 1.0%)"]
        );
    }

    #[test]
    fn soft_caps_alone_are_no_ceiling_and_mixed_chains_use_the_hard_ones() {
        let soft_only = vec![
            job(1, soft(1000), Nesting::TopLevel),
            job(2, soft(2000), Nesting::Nested(1)),
        ];
        assert_eq!(
            only(soft_only),
            vec!["job 2 machine share: no hard ceiling (only soft CPU caps in the chain)"]
        );
        let mixed = vec![
            job(1, hard(1000), Nesting::TopLevel),
            job(2, soft(2000), Nesting::Nested(1)),
        ];
        assert_eq!(
            only(mixed),
            vec!["job 2 machine share: at most 10.0% all / 160.0% one (10.0%)"]
        );
    }

    #[test]
    fn an_unproven_chain_is_a_question_mark() {
        let unnamed = vec![
            job(1, hard(1000), Nesting::TopLevel),
            job(2, hard(2000), Nesting::NestedUnnamed),
        ];
        assert_eq!(only(unnamed), vec!["job 2 machine share: ?"]);
        let unknown_nesting = vec![job(1, hard(2000), Nesting::Unknown)];
        assert_eq!(only(unknown_nesting), vec!["job 1 machine share: ?"]);
        let unknown_cap = vec![
            job(1, CapState::Unknown, Nesting::TopLevel),
            job(2, hard(2000), Nesting::Nested(1)),
        ];
        assert_eq!(only(unknown_cap), vec!["job 2 machine share: ?"]);
        let missing_parent = vec![job(2, hard(2000), Nesting::Nested(9))];
        assert_eq!(only(missing_parent), vec!["job 2 machine share: ?"]);
        let cycle = vec![
            job(1, hard(1000), Nesting::Nested(2)),
            job(2, hard(1000), Nesting::Nested(1)),
        ];
        assert_eq!(
            only(cycle),
            vec!["job 1 machine share: ?", "job 2 machine share: ?"]
        );
    }

    #[test]
    fn render_puts_the_lines_after_the_legend_and_before_processes() {
        let mut a = job(1, hard(1000), Nesting::TopLevel);
        a.owner_command = Some("x".repeat(COMMAND_WIDTH + 1));
        let b = job(2, hard(2000), Nesting::Nested(1));
        let text = super::super::render(&scan(vec![a, b]));
        let rows: Vec<&str> = text.lines().collect();
        let legend = rows
            .iter()
            .position(|l| l.starts_with("note: nested"))
            .unwrap();
        assert_eq!(rows[legend + 1], "");
        assert!(rows[legend + 2].starts_with("job 1 command: "));
        assert!(rows[legend + 3].starts_with("job 2 machine share: "));
        assert_eq!(rows[legend + 4], "");
        assert!(rows[legend + 5].starts_with("PROCESSES: "));
        assert!(rows.iter().all(|l| !l.ends_with(' ')));
    }
}
