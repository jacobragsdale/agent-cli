use agent_cli_core::{Ctx, command};
use anyhow::Result;

use super::{AnswerArgs, Answered, answer};

fn approval_approve(ctx: &Ctx, args: AnswerArgs) -> Result<Answered> {
    answer(ctx, args, "approved")
}

command! {
    pub APPROVAL_APPROVE = ["ado", "approval", "approve"], Destructive,
    "Approve a pending pipeline approval, letting the stage (a deploy) run",
    keywords: ["gate", "deploy", "release", "allow", "production", "sign", "off"],
    example: "ado approval approve 1f2e3d4c-0000-4000-8000-000000000001 --comment 'Checked staging' --yes",
    run: approval_approve,
}
