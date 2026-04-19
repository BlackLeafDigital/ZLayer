
# CronStatusResponse

Response for enable/disable operations

## Properties

Name | Type
------------ | -------------
`enabled` | boolean
`message` | string
`name` | string

## Example

```typescript
import type { CronStatusResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "enabled": null,
  "message": null,
  "name": null,
} satisfies CronStatusResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as CronStatusResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


