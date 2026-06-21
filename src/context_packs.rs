pub struct ContextPack {
    pub name: &'static str,
    pub content: &'static str,
    pub triggers: &'static [&'static str],
}

pub static PROMPT_INJECTION: ContextPack = ContextPack {
    name: "prompt_injection",
    content: "\
Known prompt injection and jailbreak signatures: \
override or ignore previous instructions, \"developer mode\", jailbreak framings, \
base64-encoded instruction blocks, nested system prompt injection, \
\"pretend you are\" / \"you are now\" / \"you are no longer\" role-replacement, \
instruction leakage requests (asking the model to repeat its prompt), \
adversarial roleplay designed to suppress model constraints.",
    triggers: &[
        "ignore previous",
        "ignore all previous",
        "override",
        "jailbreak",
        "developer mode",
        "base64",
        "system prompt",
        "pretend you are",
        "you are now",
        "you are no longer",
        "disregard",
        "forget your",
        "new instructions",
        "repeat your",
    ],
};

pub static SOCIAL_ENGINEERING: ContextPack = ContextPack {
    name: "social_engineering",
    content: "\
Social engineering manipulation tactics: \
authority impersonation (CEO, IT, legal, government, security team), \
manufactured urgency (\"act now\", \"time-sensitive\", \"immediately\"), \
artificial scarcity or threat of account suspension, \
credential or wire-transfer harvesting disguised as routine verification, \
pretexting (false backstory to establish trust before a demand), \
fear/uncertainty/doubt (FUD) amplification, \
quid pro quo offers (something for compliance).",
    triggers: &[
        "ceo",
        "executive",
        "wire transfer",
        "invoice",
        "suspended",
        "penalty",
        "lawsuit",
        "immediately",
        "act now",
        "time sensitive",
        "time-sensitive",
        "confidential",
        "your account",
        "verify your",
        "update your",
        "click here",
    ],
};

pub static EMOTIONAL_MANIPULATION: ContextPack = ContextPack {
    name: "emotional_manipulation",
    content: "\
Emotional manipulation patterns: \
guilt induction (making the target feel responsible for negative consequences), \
flattery bombing followed by high-demand requests, \
victimhood leveraging to lower critical defenses, \
manufactured crisis or catastrophizing to overwhelm rational evaluation, \
intermittent reinforcement (alternating reward and punishment), \
gaslighting (undermining the target's perception of events), \
isolation framing (\"you're the only one who can help\").",
    triggers: &[
        "desperate",
        "abandoned",
        "your fault",
        "blame you",
        "disappointed in",
        "you made me",
        "you don't care",
        "nobody cares",
        "you're the only",
        "you are the only",
        "if you don't",
        "you'll regret",
        "you will regret",
        "i'm suffering",
        "i am suffering",
        "never forgive",
    ],
};

pub static ADVERSARIAL_PROBING: ContextPack = ContextPack {
    name: "adversarial_probing",
    content: "\
Adversarial system-probing patterns: \
capability elicitation (asking what the system can or cannot do), \
boundary testing (probing refusal and override conditions), \
prompt/instruction extraction (asking the model to reveal its instructions), \
confusion injection (contradictory inputs to cause errors), \
meta-level instruction injection (treating model output as executable), \
multi-turn escalation (building context across messages to gradually shift behavior).",
    triggers: &[
        "your instructions",
        "your system prompt",
        "print your",
        "output your",
        "show me your",
        "what are your rules",
        "what are your limitations",
        "your limitations",
        "bypass",
        "forbidden",
        "what can you not",
        "what are you not",
        "reveal your",
        "expose your",
    ],
};

pub fn all_packs() -> &'static [&'static ContextPack] {
    static PACKS: [&ContextPack; 4] = [
        &PROMPT_INJECTION,
        &SOCIAL_ENGINEERING,
        &EMOTIONAL_MANIPULATION,
        &ADVERSARIAL_PROBING,
    ];
    &PACKS
}
