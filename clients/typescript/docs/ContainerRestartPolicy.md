
# ContainerRestartPolicy

Container-runtime-level restart policy.  Maps onto Docker\'s `HostConfig.RestartPolicy`. Distinct from [`PanicPolicy`], which governs what `ZLayer` does in response to an application panic (it does not set a Docker restart policy).

## Properties

Name | Type
------------ | -------------
`delay` | string
`kind` | [ContainerRestartKind](ContainerRestartKind.md)
`maxAttempts` | number

## Example

```typescript
import type { ContainerRestartPolicy } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "delay": null,
  "kind": null,
  "maxAttempts": null,
} satisfies ContainerRestartPolicy

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerRestartPolicy
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


