
# WebhookInfoResponse

Response for `GET /api/v1/projects/{id}/webhook`.

## Properties

Name | Type
------------ | -------------
`secret` | string
`url` | string

## Example

```typescript
import type { WebhookInfoResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "secret": null,
  "url": null,
} satisfies WebhookInfoResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as WebhookInfoResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


