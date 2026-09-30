use agent_cli_core::{Ctx, command};
use anyhow::Result;

use super::{AnswerArgs, Answered, answer};

fn approval_reject(ctx: &Ctx, args: AnswerArgs) -> Result<Answered> {
    answer(ctx, args, "rejected")
}

command! {
    pub APPROVAL_REJECT = ["ado", "approval", "reject"], Destructive,
    "Reject a pending pipeline approval, stopping the stage",
    keywords: ["gate", "deploy", "release", "deny", "block", "production"],
    example: "ado approval reject 1f2e3d4c-0000-4000-8000-000000000001 --comment 'Not today' --yes",
    run: approval_reject,
}
