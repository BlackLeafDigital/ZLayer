
# PruneResultDto

Serializable wrapper for [`PruneResult`].

## Properties

Name | Type
------------ | -------------
`deleted` | Array&lt;string&gt;
`spaceReclaimed` | number

## Example

```typescript
import type { PruneResultDto } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "deleted": null,
  "spaceReclaimed": null,
} satisfies PruneResultDto

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as PruneResultDto
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


