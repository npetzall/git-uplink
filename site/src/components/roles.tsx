import { ArrowRight } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "../lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";

const ROLES = [
  {
    title: "Developer",
    href: "/developer",
    body: "Branches from company main, opens a pull request, merges. Works in the gated pull requests the forge opens, and dispatches amend, transfer and submit. Never runs git uplink.",
  },
  {
    title: "Forge",
    href: "/forge",
    body: "The workflows of the pack. Imports merged pull requests, syncs upstream, submits, rebases, and gates conflicts, amends and transfers through pull requests.",
  },
  {
    title: "Operator",
    href: "/operator",
    body: "Sets the repositories up, runs init and upgrades, adopts existing history, troubleshoots, and repairs by hand when the forge cannot.",
  },
];

/** The three roles, with a card each linking to its page. */
export function RolesSection({ className }: { className?: string }) {
  return (
    <section className={cn("space-y-4", className)}>
      <h2 className="text-xl font-semibold">Three roles</h2>
      <p className="max-w-3xl text-sm leading-6 text-muted-foreground">
        Each role has its own job, and none does another&apos;s. A developer
        does not rewrite a patch, the operator does not stand in for the forge,
        and the forge never touches a developer&apos;s branch unless asked.
      </p>
      <div className="grid gap-4 md:grid-cols-3">
        {ROLES.map((role) => (
          <Card key={role.title}>
            <CardHeader>
              <CardTitle className="text-base">
                <Link
                  to={role.href}
                  className="inline-flex items-center gap-1 hover:text-primary"
                >
                  {role.title} <ArrowRight className="size-4" />
                </Link>
              </CardTitle>
            </CardHeader>
            <CardContent className="text-sm leading-6 text-muted-foreground">
              {role.body}
            </CardContent>
          </Card>
        ))}
      </div>
    </section>
  );
}
