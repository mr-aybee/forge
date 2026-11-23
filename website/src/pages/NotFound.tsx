import { Button, Container } from "../components/ui";

export function NotFound() {
  return (
    <Container className="flex flex-col items-center py-32 text-center">
      <p className="font-mono text-sm text-accent">404</p>
      <h1 className="mt-3 text-4xl font-semibold tracking-tight">This page does not exist.</h1>
      <p className="mt-4 text-muted">It may have moved. Try the home page or the documentation.</p>
      <div className="mt-8 flex gap-3">
        <Button href="/">Home</Button>
        <Button href="/docs/" variant="secondary">Docs</Button>
      </div>
    </Container>
  );
}
