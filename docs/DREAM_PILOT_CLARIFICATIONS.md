# Implementation clarification before any live run

The registration commit is `2fea927176ecaa3337afc716d0685d792ad5b323`.
Its design, preregistration and task fixture remain unchanged.

A rejected optional repair description is not a measurement outcome. Once all
evaluation arms finish, repair content rejected for schema, privacy, size or
oracle admission yields no repair and a sanitized content-admission trace note.
The research run remains Completed and retains every measured result and spent
call. Rejected text is never retained. This applies to the default controller:
admitting a proposed artifact should not erase already observed evidence.
Hypothesis and measurement admission behavior is unchanged.

Provider/transport failures, timeout, cancellation and budget failures during
repair generation still fail/abort the experiment and pilot. This clarification
implements the registered rule that repair prose is not an outcome, without
changing case selection, scoring, thresholds, budget or stop rules. It must be
committed with the reviewed implementation before any live execution.
