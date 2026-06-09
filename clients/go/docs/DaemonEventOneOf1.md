# DaemonEventOneOf1

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time. | 
**Digest** | Pointer to **string** | Optional content digest (&#x60;sha256:...&#x60;) when known. | [optional] 
**Kind** | [**ImageEventKind**](ImageEventKind.md) |  | 
**Reference** | **string** | Image reference (e.g. &#x60;nginx:latest&#x60; or &#x60;registry/foo/bar@sha256:...&#x60;). | 
**Source** | Pointer to **string** | Source reference for &#x60;Tag&#x60; events (the image being aliased). | [optional] 
**Resource** | **string** |  | 

## Methods

### NewDaemonEventOneOf1

`func NewDaemonEventOneOf1(at time.Time, kind ImageEventKind, reference string, resource string, ) *DaemonEventOneOf1`

NewDaemonEventOneOf1 instantiates a new DaemonEventOneOf1 object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDaemonEventOneOf1WithDefaults

`func NewDaemonEventOneOf1WithDefaults() *DaemonEventOneOf1`

NewDaemonEventOneOf1WithDefaults instantiates a new DaemonEventOneOf1 object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *DaemonEventOneOf1) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *DaemonEventOneOf1) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *DaemonEventOneOf1) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetDigest

`func (o *DaemonEventOneOf1) GetDigest() string`

GetDigest returns the Digest field if non-nil, zero value otherwise.

### GetDigestOk

`func (o *DaemonEventOneOf1) GetDigestOk() (*string, bool)`

GetDigestOk returns a tuple with the Digest field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDigest

`func (o *DaemonEventOneOf1) SetDigest(v string)`

SetDigest sets Digest field to given value.

### HasDigest

`func (o *DaemonEventOneOf1) HasDigest() bool`

HasDigest returns a boolean if a field has been set.

### GetKind

`func (o *DaemonEventOneOf1) GetKind() ImageEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *DaemonEventOneOf1) GetKindOk() (*ImageEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *DaemonEventOneOf1) SetKind(v ImageEventKind)`

SetKind sets Kind field to given value.


### GetReference

`func (o *DaemonEventOneOf1) GetReference() string`

GetReference returns the Reference field if non-nil, zero value otherwise.

### GetReferenceOk

`func (o *DaemonEventOneOf1) GetReferenceOk() (*string, bool)`

GetReferenceOk returns a tuple with the Reference field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReference

`func (o *DaemonEventOneOf1) SetReference(v string)`

SetReference sets Reference field to given value.


### GetSource

`func (o *DaemonEventOneOf1) GetSource() string`

GetSource returns the Source field if non-nil, zero value otherwise.

### GetSourceOk

`func (o *DaemonEventOneOf1) GetSourceOk() (*string, bool)`

GetSourceOk returns a tuple with the Source field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSource

`func (o *DaemonEventOneOf1) SetSource(v string)`

SetSource sets Source field to given value.

### HasSource

`func (o *DaemonEventOneOf1) HasSource() bool`

HasSource returns a boolean if a field has been set.

### GetResource

`func (o *DaemonEventOneOf1) GetResource() string`

GetResource returns the Resource field if non-nil, zero value otherwise.

### GetResourceOk

`func (o *DaemonEventOneOf1) GetResourceOk() (*string, bool)`

GetResourceOk returns a tuple with the Resource field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResource

`func (o *DaemonEventOneOf1) SetResource(v string)`

SetResource sets Resource field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


