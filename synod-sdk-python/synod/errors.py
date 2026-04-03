"""Synod Agent SDK — error types."""


class SynodError(Exception):
    """Base error for all Synod SDK errors."""
    pass


class ConnectTimeoutError(SynodError):
    """User did not approve signer authorization in time."""
    pass


class SynodConnectionError(SynodError):
    """Network / coordinator unreachable."""

    def __init__(self, message: str, retry_after: float | None = None):
        super().__init__(message)
        self.retry_after = retry_after


class PermitDeniedError(SynodError):
    """Permit request was denied by the policy engine."""

    def __init__(self, reason: str, policy_check: int | None = None):
        super().__init__(f"Permit denied: {reason}")
        self.reason = reason
        self.policy_check = policy_check


class PartialApprovalError(SynodError):
    """Approved amount < requested amount and reject_partial=True."""

    def __init__(self, approved_amount: float, requested_amount: float):
        super().__init__(
            f"Partial approval: {approved_amount} of {requested_amount} approved"
        )
        self.approved_amount = approved_amount
        self.requested_amount = requested_amount


class TreasuryHaltedError(SynodError):
    """Treasury is in emergency stop — all operations paused."""
    pass


class AgentSuspendedError(SynodError):
    """This agent has been suspended by the treasury owner."""
    pass


class AgentNotActiveError(SynodError):
    """Agent is not yet active and cannot execute treasury actions."""

    def __init__(self, reason_code: str | None = None):
        message = "Agent is not active"
        if reason_code:
            message = f"{message}: {reason_code}"
        super().__init__(message)
        self.reason_code = reason_code


class AllocationLimitError(SynodError):
    """Agent's allocation limit would be exceeded."""

    def __init__(self, agent_max_usd: float, current_reserved_usd: float, requested: float):
        super().__init__(
            f"Allocation limit: max={agent_max_usd}, reserved={current_reserved_usd}, requested={requested}"
        )
        self.agent_max_usd = agent_max_usd
        self.current_reserved_usd = current_reserved_usd
        self.requested = requested


class TierLimitError(SynodError):
    """Single-transaction tier limit exceeded."""

    def __init__(self, tier_limit_usd: float, requested: float):
        super().__init__(f"Tier limit: max={tier_limit_usd}, requested={requested}")
        self.tier_limit_usd = tier_limit_usd
        self.requested = requested


class ConcurrentLimitError(SynodError):
    """Too many active permits — must wait for capacity."""

    def __init__(self, retry_after_seconds: float = 5.0):
        super().__init__("Concurrent permit limit reached")
        self.retry_after_seconds = retry_after_seconds


class DrawdownLimitError(SynodError):
    """Treasury-wide drawdown limit hit."""
    pass


class WalletNotAssignedError(SynodError):
    """Agent tried to use a wallet it has no access to."""
    pass
