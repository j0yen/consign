//! consign verify — convergence check after a drain run.
//!
//! Re-surveys the fleet and cross-checks against a drain receipt to prove
//! push-debt actually reached zero for every repo drain claimed to have pushed.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::drain::{classify, DrainReceipt, PolicyDecision};
use crate::{survey, DebtClass};

// ---------------------------------------------------------------------------
// Verdict types
// ---------------------------------------------------------------------------

/// Per-repo verdict after cross-checking against a drain receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepoVerdict {
    /// Repo is clean (no push-debt).
    Clean,
    /// Repo was claimed pushed in drain receipt but is still ahead/no-upstream.
    Contradicted,
    /// Repo has auto-ok debt but was not in the drain receipt (or no receipt provided).
    Residual,
    /// Repo is out of scope (manual-only/diverged/private-hold/no-remote).
    OutOfScope,
}

/// Per-repo verification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoVerification {
    /// Repo name (last path component).
    pub name: String,
    /// Absolute path to the repo.
    pub path: PathBuf,
    /// Current branch.
    pub branch: String,
    /// Current debt class from fresh survey.
    pub current_class: String,
    /// Verdict for this repo.
    pub verdict: RepoVerdict,
    /// Human-readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Overall fleet verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverallVerdict {
    /// All auto-ok push-debt is gone (exit 0).
    Converged,
    /// A repo drain claimed pushed is still ahead/no-upstream (exit 1).
    Contradicted,
    /// Auto-ok debt remains but no drain receipt was provided to contradict it (exit 0 + warning).
    Residual,
}

impl OverallVerdict {
    /// Human-readable description of the verdict and its exit code.
    pub fn description(&self) -> &'static str {
        match self {
            OverallVerdict::Converged => "converged — fleet is clean (exit 0)",
            OverallVerdict::Contradicted => {
                "contradicted — drain claimed repos pushed but debt remains (exit 1)"
            }
            OverallVerdict::Residual => {
                "residual — auto-ok debt remains; no drain receipt to contradict (exit 0, warning)"
            }
        }
    }

    /// The process exit code for this verdict.
    pub fn exit_code(&self) -> i32 {
        match self {
            OverallVerdict::Converged => 0,
            OverallVerdict::Contradicted => 1,
            OverallVerdict::Residual => 0,
        }
    }
}

/// Receipt produced by `verify()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReceipt {
    /// Overall fleet verdict.
    pub overall: OverallVerdict,
    /// Per-repo verification results.
    pub repos: Vec<RepoVerification>,
    /// Repos whose verdict is `Contradicted`.
    pub contradictions: Vec<String>,
    /// Repos with residual auto-ok debt.
    pub residuals: Vec<String>,
    /// Whether the survey was run (always true for a real verify; used in tests).
    pub survey_ran: bool,
}

// ---------------------------------------------------------------------------
// Drain receipt deserialization (subset of DrainReceipt fields we need)
// ---------------------------------------------------------------------------

/// Subset of drain receipt fields needed for cross-checking.
/// The full DrainReceipt struct is in drain.rs; we import it directly.
fn repo_name_from_path(path: &PathBuf) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Returns true if a `DebtClass` indicates the repo is still "ahead" in push terms.
fn is_still_ahead(class: &DebtClass) -> bool {
    matches!(class, DebtClass::Ahead { .. } | DebtClass::NoUpstream { .. })
}

/// Returns true if a debt class is out of convergence scope:
/// manual-only (Diverged), no-remote, already clean.
fn is_out_of_scope(policy: &PolicyDecision) -> bool {
    matches!(
        policy,
        PolicyDecision::ManualOnly | PolicyDecision::NoRemote | PolicyDecision::Clean
    )
}

// ---------------------------------------------------------------------------
// Main verify logic
// ---------------------------------------------------------------------------

/// Configuration for a verify run.
pub struct VerifyConfig {
    /// Root directories to scan.
    pub roots: Vec<PathBuf>,
    /// Drain receipt to cross-check against, if any.
    pub drain_receipt: Option<Vec<DrainReceipt>>,
}

/// Run a verify pass: survey the fleet, optionally cross-check against a drain receipt.
///
/// Returns a `VerifyReceipt` describing the convergence state.
/// The caller is responsible for using `receipt.overall.exit_code()` to set the
/// process exit code.
pub fn verify(config: &VerifyConfig) -> Result<VerifyReceipt, crate::SurveyError> {
    // AC5: always run a fresh survey, even when --against is supplied.
    let debts = survey(&config.roots)?;

    // Build a set of repo names that drain claimed to have successfully pushed.
    // A receipt entry counts as "claimed pushed" if:
    //   - result == Ok (not DryRun or Error)
    //   - action is Push or SetUpstream
    let claimed_pushed: std::collections::HashSet<String> = config
        .drain_receipt
        .as_deref()
        .map(|receipts| {
            receipts
                .iter()
                .filter(|r| {
                    r.result == crate::drain::ActionResult::Ok
                        && matches!(
                            r.action,
                            crate::drain::DrainAction::Push | crate::drain::DrainAction::SetUpstream
                        )
                })
                .map(|r| r.name.clone())
                .collect()
        })
        .unwrap_or_default();

    let has_receipt = config.drain_receipt.is_some();

    let mut repos: Vec<RepoVerification> = Vec::new();
    let mut any_contradicted = false;
    let mut any_residual = false;

    for debt in &debts {
        let name = repo_name_from_path(&debt.path);
        let policy = classify(debt);

        let verdict = if is_out_of_scope(&policy) {
            // Diverged / no-remote / clean — out of convergence scope.
            // Clean is technically "converged" but we mark it separately here;
            // it does not contribute to any_residual or any_contradicted.
            RepoVerdict::OutOfScope
        } else {
            // auto-ok: Ahead or NoUpstream
            if is_still_ahead(&debt.class) {
                if has_receipt && claimed_pushed.contains(&name) {
                    // Drain said it pushed this repo but it's still ahead — contradiction.
                    any_contradicted = true;
                    RepoVerdict::Contradicted
                } else {
                    // Still ahead with no receipt claiming it was pushed — residual.
                    any_residual = true;
                    RepoVerdict::Residual
                }
            } else {
                // auto-ok and now clean — converged for this repo.
                RepoVerdict::Clean
            }
        };

        let detail = match &verdict {
            RepoVerdict::Contradicted => Some(format!(
                "drain claimed pushed but still {}",
                debt.class.name()
            )),
            RepoVerdict::Residual => Some(format!(
                "{} (auto-ok debt, no drain receipt)",
                debt.class.name()
            )),
            RepoVerdict::OutOfScope => Some(format!("{} — out of convergence scope", debt.class.name())),
            RepoVerdict::Clean => None,
        };

        repos.push(RepoVerification {
            name,
            path: debt.path.clone(),
            branch: debt.branch.clone(),
            current_class: debt.class.name().to_string(),
            verdict,
            detail,
        });
    }

    let overall = if any_contradicted {
        OverallVerdict::Contradicted
    } else if any_residual {
        OverallVerdict::Residual
    } else {
        OverallVerdict::Converged
    };

    let contradictions: Vec<String> = repos
        .iter()
        .filter(|r| r.verdict == RepoVerdict::Contradicted)
        .map(|r| r.name.clone())
        .collect();

    let residuals: Vec<String> = repos
        .iter()
        .filter(|r| r.verdict == RepoVerdict::Residual)
        .map(|r| r.name.clone())
        .collect();

    Ok(VerifyReceipt {
        overall,
        repos,
        contradictions,
        residuals,
        survey_ran: true,
    })
}

/// Render a `VerifyReceipt` as a human-readable table.
pub fn render_verify_table(receipt: &VerifyReceipt) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let name_w = receipt
        .repos
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let verdict_w = "contradicted".len(); // longest verdict name

    let _ = writeln!(
        out,
        "{:<name_w$}  {:<verdict_w$}  detail",
        "REPO",
        "VERDICT",
        name_w = name_w,
        verdict_w = verdict_w,
    );
    let sep = format!("{}  {}  {}", "-".repeat(name_w), "-".repeat(verdict_w), "------");
    let _ = writeln!(out, "{}", sep);

    for r in &receipt.repos {
        let verdict_str = match r.verdict {
            RepoVerdict::Clean => "clean",
            RepoVerdict::Contradicted => "contradicted",
            RepoVerdict::Residual => "residual",
            RepoVerdict::OutOfScope => "out-of-scope",
        };
        let _ = writeln!(
            out,
            "{:<name_w$}  {:<verdict_w$}  {}",
            r.name,
            verdict_str,
            r.detail.as_deref().unwrap_or(""),
            name_w = name_w,
            verdict_w = verdict_w,
        );
    }

    let _ = writeln!(out, "{}", sep);
    let _ = writeln!(out, "Overall: {}", receipt.overall.description());

    if !receipt.contradictions.is_empty() {
        let _ = writeln!(out, "\nContradictions:");
        for name in &receipt.contradictions {
            let _ = writeln!(out, "  - {}", name);
        }
    }

    if !receipt.residuals.is_empty() {
        let _ = writeln!(out, "\nWarning: residual auto-ok debt (run consign drain to clear):");
        for name in &receipt.residuals {
            let _ = writeln!(out, "  - {}", name);
        }
    }

    out
}
