"""gate.py の期待値判定のテスト。"""

import unittest

from scripts.ci import gate


def needs(plan="success", ci="success", policy="success"):
    return {
        "plan": {"result": plan},
        "ci": {"result": ci},
        "policy": {"result": policy},
    }


class GateTests(unittest.TestCase):
    def test_passing_pull_request_has_no_problems(self):
        self.assertEqual(gate.errors(["repository"], needs(), 1), [])

    def test_plan_failure_is_reported(self):
        self.assertEqual(
            gate.errors([], needs(plan="failure", ci="skipped", policy="skipped"), 0, "push"),
            ["plan did not succeed"],
        )

    def test_unknown_route_is_invalid_routing(self):
        problems = gate.errors(["nope"], needs(), 1)
        self.assertIn("invalid path routing output", problems)

    def test_count_must_match_selection(self):
        problems = gate.errors(["repository", "host"], needs(), 1)
        self.assertTrue(any("reported count=1" in problem for problem in problems))

    def test_selected_routes_must_succeed(self):
        problems = gate.errors(["repository"], needs(ci="failure"), 1)
        self.assertIn("ci: expected success, got failure", problems)

    def test_nothing_selected_must_be_skipped(self):
        self.assertEqual(gate.errors([], needs(ci="skipped", policy="skipped"), 0, "push"), [])
        problems = gate.errors([], needs(ci="success", policy="skipped"), 0, "push")
        self.assertIn("ci: expected skipped, got success", problems)

    def test_policy_must_pass_on_pull_requests(self):
        problems = gate.errors(["repository"], needs(policy="failure"), 1, "pull_request")
        self.assertIn("policy: expected success, got failure", problems)

    def test_policy_is_skipped_outside_pull_requests(self):
        self.assertEqual(
            gate.errors(["repository"], needs(policy="skipped"), 1, "push"), []
        )
        problems = gate.errors(["repository"], needs(policy="success"), 1, "workflow_dispatch")
        self.assertIn("policy: expected skipped, got success", problems)


if __name__ == "__main__":
    unittest.main()
