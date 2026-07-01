#!/usr/bin/env python3
import json
import sys


CUSTOMERS = {
    "cus_123": {
        "customer_id": "cus_123",
        "name": "Acme Corp",
        "tier": "enterprise",
        "health": "at_risk",
        "renewal_date": "2026-09-30",
        "owner": "sam@example.com",
        "open_risks": [
            "Usage dropped 24% over the last 30 days",
            "Two unresolved support tickets mention onboarding delays"
        ]
    }
}

ORDERS = {
    "cus_123": [
        {
            "order_id": "ord_1001",
            "status": "completed",
            "total_usd": 12900,
            "created_at": "2026-06-20T15:18:00Z"
        },
        {
            "order_id": "ord_1002",
            "status": "pending",
            "total_usd": 4200,
            "created_at": "2026-06-27T09:05:00Z"
        }
    ]
}


def respond(request_id, result=None, error=None):
    payload = {"jsonrpc": "2.0", "id": request_id}
    if error is not None:
        payload["error"] = error
    else:
        payload["result"] = result
    print(json.dumps(payload), flush=True)


def tool_error(code, message):
    return {"code": code, "message": message}


def call_tool(name, input_value):
    if name == "get_customer_profile":
        customer_id = input_value.get("customer_id")
        profile = CUSTOMERS.get(customer_id)
        if profile is None:
            raise ValueError(f"unknown customer_id: {customer_id}")
        return {"profile": profile}

    if name == "list_recent_orders":
        customer_id = input_value.get("customer_id")
        limit = int(input_value.get("limit", 5))
        return {"orders": ORDERS.get(customer_id, [])[:limit]}

    if name == "apply_followup_email":
        return {
            "status": "accepted",
            "demo_only": True,
            "message": "Production apps should send this through their confirmed CRM/email workflow.",
            "accepted_payload": input_value
        }

    raise ValueError(f"unknown tool: {name}")


def main():
    line = sys.stdin.readline()
    if not line:
        return

    try:
        request = json.loads(line)
        params = request.get("params", {})
        result = call_tool(params.get("name"), params.get("input", {}))
        respond(request.get("id"), result=result)
    except Exception as exc:
        respond(None, error=tool_error("tool_host_error", str(exc)))


if __name__ == "__main__":
    main()
