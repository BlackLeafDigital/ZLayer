# ImageEvent

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time. | 
**Digest** | Pointer to **NullableString** | Optional content digest (&#x60;sha256:...&#x60;) when known. | [optional] 
**Kind** | [**ImageEventKind**](ImageEventKind.md) |  | 
**Reference** | **string** | Image reference (e.g. &#x60;nginx:latest&#x60; or &#x60;registry/foo/bar@sha256:...&#x60;). | 
**Source** | Pointer to **NullableString** | Source reference for &#x60;Tag&#x60; events (the image being aliased). | [optional] 

## Methods

### NewImageEvent

`func NewImageEvent(at time.Time, kind ImageEventKind, reference string, ) *ImageEvent`

NewImageEvent instantiates a new ImageEvent object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewImageEventWithDefaults

`func NewImageEventWithDefaults() *ImageEvent`

NewImageEventWithDefaults instantiates a new ImageEvent object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *ImageEvent) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *ImageEvent) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *ImageEvent) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetDigest

`func (o *ImageEvent) GetDigest() string`

GetDigest returns the Digest field if non-nil, zero value otherwise.

### GetDigestOk

`func (o *ImageEvent) GetDigestOk() (*string, bool)`

GetDigestOk returns a tuple with the Digest field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDigest

`func (o *ImageEvent) SetDigest(v string)`

SetDigest sets Digest field to given value.

### HasDigest

`func (o *ImageEvent) HasDigest() bool`

HasDigest returns a boolean if a field has been set.

### SetDigestNil

`func (o *ImageEvent) SetDigestNil(b bool)`

 SetDigestNil sets the value for Digest to be an explicit nil

### UnsetDigest
`func (o *ImageEvent) UnsetDigest()`

UnsetDigest ensures that no value is present for Digest, not even an explicit nil
### GetKind

`func (o *ImageEvent) GetKind() ImageEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *ImageEvent) GetKindOk() (*ImageEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *ImageEvent) SetKind(v ImageEventKind)`

SetKind sets Kind field to given value.


### GetReference

`func (o *ImageEvent) GetReference() string`

GetReference returns the Reference field if non-nil, zero value otherwise.

### GetReferenceOk

`func (o *ImageEvent) GetReferenceOk() (*string, bool)`

GetReferenceOk returns a tuple with the Reference field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReference

`func (o *ImageEvent) SetReference(v string)`

SetReference sets Reference field to given value.


### GetSource

`func (o *ImageEvent) GetSource() string`

GetSource returns the Source field if non-nil, zero value otherwise.

### GetSourceOk

`func (o *ImageEvent) GetSourceOk() (*string, bool)`

GetSourceOk returns a tuple with the Source field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSource

`func (o *ImageEvent) SetSource(v string)`

SetSource sets Source field to given value.

### HasSource

`func (o *ImageEvent) HasSource() bool`

HasSource returns a boolean if a field has been set.

### SetSourceNil

`func (o *ImageEvent) SetSourceNil(b bool)`

 SetSourceNil sets the value for Source to be an explicit nil

### UnsetSource
`func (o *ImageEvent) UnsetSource()`

UnsetSource ensures that no value is present for Source, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


